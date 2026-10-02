#![cfg(all(windows, feature = "broker"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use retype_learning::{protocol::*, transport};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Broker {
    child: Child,
    root: PathBuf,
    pipe: String,
}
impl Broker {
    fn start() -> Self {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("retype-ipc-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        transport::private_directory(&root.join("learning"), &transport::user_sid().unwrap())
            .unwrap();
        let pipe = format!(r"\\.\pipe\retype-learning-test-{id}");
        let child = Command::new(env!("CARGO_BIN_EXE_retype-learning-host"))
            .args([
                "--db",
                root.join("learning/user.db").to_str().unwrap(),
                "--pipe",
                &pipe,
                "--error-log",
                root.join("error.txt").to_str().unwrap(),
            ])
            .creation_flags(0x08000000)
            .spawn()
            .unwrap();
        let mut broker = Self { child, root, pipe };
        broker.wait_ready();
        broker
    }
    fn wait_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if self.call(&Self::poll("ready")).is_ok() {
                return;
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!(
                    "broker exited {status}: {:?}",
                    std::fs::read_to_string(self.root.join("error.txt"))
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("broker did not become ready");
    }
    fn poll(client: &str) -> Request {
        Request {
            version: VERSION,
            client: client.into(),
            known_revision: None,
            events: vec![],
            stop: false,
        }
    }
    fn call(&self, request: &Request) -> std::io::Result<Response> {
        let bytes = transport::exchange(&self.pipe, &serde_json::to_vec(request)?)?;
        Ok(serde_json::from_slice(&bytes)?)
    }
    fn restart(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        self.restart_after_exit();
    }
    fn restart_after_exit(&mut self) {
        self.child = Command::new(env!("CARGO_BIN_EXE_retype-learning-host"))
            .args([
                "--db",
                self.root.join("learning/user.db").to_str().unwrap(),
                "--pipe",
                &self.pipe,
            ])
            .creation_flags(0x08000000)
            .spawn()
            .unwrap();
        self.wait_ready();
    }
}
impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn real_pipe_survives_broker_crash_and_replayed_requests() {
    let mut broker = Broker::start();
    let mut event = Broker::poll("notepad");
    event.events.push(SequencedEvent {
        sequence: 1,
        event: Event::Chosen {
            text: "你好".into(),
            pinyin: "ni'hao".into(),
            index: 3,
        },
    });
    let first = broker.call(&event).unwrap();
    assert_eq!(first.acknowledged, 1);
    let score = first.snapshot.unwrap().entries[0].logp;
    broker.restart();
    let replay = broker.call(&event).unwrap();
    assert_eq!(replay.revision, 1);
    assert_eq!(replay.snapshot.unwrap().entries[0].logp, score);
    let other = broker.call(&Broker::poll("search")).unwrap();
    assert_eq!(other.snapshot.unwrap().entries[0].text, "你好");
}

#[test]
fn cloud_learning_merge_and_export_use_the_real_authenticated_pipe() {
    let mut broker = Broker::start();
    let exchange = |broker: &Broker, request: &SyncRequest| -> SyncLearning {
        let bytes =
            transport::exchange(&broker.pipe, &serde_json::to_vec(request).unwrap()).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    };
    let incoming = SyncLearning {
        words: vec![SyncWord {
            origin: "remote-fixture".into(),
            pinyin: "ni'hao".into(),
            text: "你好".into(),
            count: 7,
        }],
        corrections: vec![],
    };
    let first = exchange(&broker, &SyncRequest::Merge(incoming.clone()));
    assert_eq!(first.words[0].count, 7);
    let again = exchange(&broker, &SyncRequest::Merge(incoming));
    assert_eq!(again, first);
    broker.restart();
    assert_eq!(exchange(&broker, &SyncRequest::Export), first);
    let snapshot = broker
        .call(&Broker::poll("after-cloud"))
        .unwrap()
        .snapshot
        .unwrap();
    assert_eq!(snapshot.ranking.unwrap().records[0].count, 7);
}

#[test]
fn second_broker_cannot_become_another_writer() {
    let broker = Broker::start();
    let status = Command::new(env!("CARGO_BIN_EXE_retype-learning-host"))
        .args([
            "--db",
            broker.root.join("learning/user.db").to_str().unwrap(),
            "--pipe",
            &broker.pipe,
        ])
        .creation_flags(0x08000000)
        .status()
        .unwrap();
    assert!(!status.success());
    assert!(broker.call(&Broker::poll("still-running")).is_ok());
}

#[test]
fn broken_and_oversized_clients_do_not_poison_the_next_connection() {
    use std::io::Write;
    use std::os::windows::fs::OpenOptionsExt;
    let broker = Broker::start();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut connection = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(0x00100000 | 0x00010000)
            .open(&broker.pipe)
        {
            Ok(connection) => break connection,
            Err(error) if error.raw_os_error() == Some(231) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(error) => panic!("open broken-client fixture: {error}"),
        }
    };
    connection.write_all(&u32::MAX.to_le_bytes()).unwrap();
    drop(connection);
    std::thread::sleep(Duration::from_millis(30));
    assert!(broker.call(&Broker::poll("after-invalid-client")).is_ok());
}

#[test]
fn appcontainer_child_probe() {
    let Ok(pipe) = std::env::var("RETYPE_TEST_PIPE") else {
        return;
    };
    let mut request = Broker::poll("appcontainer");
    request.events.push(SequencedEvent {
        sequence: 1,
        event: Event::Chosen {
            text: "你好".into(),
            pinyin: "ni'hao".into(),
            index: 1,
        },
    });
    let bytes = match transport::exchange(&pipe, &serde_json::to_vec(&request).unwrap()) {
        Ok(bytes) => bytes,
        Err(error) => std::process::exit(error.raw_os_error().unwrap_or(250).min(250)),
    };
    let response: Response = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(response.acknowledged, 1);
    assert!(
        std::fs::read(std::env::var("RETYPE_TEST_PRIVATE_DB").unwrap()).is_err(),
        "an AppContainer must not read the broker's word-bearing database"
    );
}

fn wait_for(condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(condition(), "asynchronous learning did not converge");
}

#[test]
fn independent_client_caches_share_and_retry_learning_after_outage() {
    use retype_learning::client::Client;
    use retype_types::LearningStore;
    use std::sync::Arc;
    fn selection() -> retype_types::LearningEvent {
        retype_types::LearningEvent::CandidateChosen {
            source: retype_types::InputSource::Keyboard,
            text: "你好".into(),
            syllables: retype_dict::annotate::parse_pinyin("ni hao").unwrap(),
            index: 3,
        }
    }
    fn score(client: &Client) -> f32 {
        if client.user.usage_snapshot().records.is_empty() {
            return -100.;
        }
        let (base, _) =
            retype_dict::from_pairs([("你好", "ni hao", 1000.), ("拟好", "ni hao", 2000.)]);
        let lex = retype_dict::LayeredDict::with_system_and_user(
            Arc::new(base),
            Arc::clone(&client.user),
            0.,
        );
        retype_pinyin::Decoder::new()
            .decode("nihao", &lex)
            .candidates
            .into_iter()
            .find(|candidate| candidate.text == "你好")
            .map(|candidate| candidate.score)
            .unwrap_or(-100.)
    }
    let mut broker = Broker::start();
    let a = Client::for_endpoint(Arc::new(retype_dict::UserDict::new()), broker.pipe.clone());
    let b = Client::for_endpoint(Arc::new(retype_dict::UserDict::new()), broker.pipe.clone());
    a.record(selection());
    wait_for(|| score(&b) > -100.);
    wait_for(|| score(&a) == score(&b));
    let first = score(&a);
    broker.child.kill().unwrap();
    broker.child.wait().unwrap();
    a.record(selection());
    wait_for(|| score(&a) > first);
    // No broker is running: local learning is still usable and the event is queued.
    broker.restart_after_exit();
    wait_for(|| score(&a) == score(&b) && score(&b) > first);
    let db = rusqlite::Connection::open(broker.root.join("learning/user.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT selections FROM entries", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    drop(a);
    drop(b);
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
fn large_snapshot_crosses_pipe_buffer_boundaries_intact() {
    let broker = Broker::start();
    let mut sequence = 0;
    for batch in 0..12 {
        let mut request = Broker::poll("large");
        for item in 0..MAX_BATCH {
            sequence += 1;
            request.events.push(SequencedEvent {
                sequence,
                event: Event::Coinage {
                    pinyin: "ni'hao".into(),
                    text: format!("测试词语-{batch}-{item}"),
                },
            });
        }
        assert_eq!(broker.call(&request).unwrap().acknowledged, sequence);
    }
    let snapshot = broker
        .call(&Broker::poll("reader"))
        .unwrap()
        .snapshot
        .unwrap();
    assert!(serde_json::to_vec(&snapshot).unwrap().len() > 65536);
    assert_eq!(snapshot.entries.len(), 12 * MAX_BATCH);
}

#[test]
#[allow(unsafe_code)]
fn appcontainer_can_learn_through_pipe_but_cannot_read_database() {
    use std::mem::size_of;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Security::Authorization::*;
    use windows_sys::Win32::Security::Isolation::*;
    use windows_sys::Win32::Security::*;
    use windows_sys::Win32::System::Threading::*;

    let broker = Broker::start();
    // Separate private DB and executable directory: the probe gets read/execute only.
    let sid = transport::user_sid().unwrap();
    let probe = broker.root.join("probe.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &probe).unwrap();
    let executable_sddl = transport::wide(&format!(
        "D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)(A;OICI;GRGX;;;AC)"
    ));
    let mut descriptor = null_mut();
    // SAFETY: granting read/execute on this isolated test directory and its probe only.
    unsafe {
        assert_ne!(
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                executable_sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut()
            ),
            0
        );
        let applied = SetFileSecurityW(
            transport::wide(&broker.root.to_string_lossy()).as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        );
        LocalFree(descriptor);
        assert_ne!(applied, 0);
        // Existing copied files do not inherit a new parent ACL via SetFileSecurityW.
        let mut executable_descriptor = null_mut();
        assert_ne!(
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                executable_sddl.as_ptr(),
                1,
                &mut executable_descriptor,
                null_mut()
            ),
            0
        );
        let applied = SetFileSecurityW(
            transport::wide(&probe.to_string_lossy()).as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            executable_descriptor,
        );
        LocalFree(executable_descriptor);
        assert_ne!(applied, 0);
    }
    let profile_name = transport::wide(&format!(
        "retype.learning.test.{}",
        broker.root.file_name().unwrap().to_string_lossy()
    ));
    let mut app_sid = null_mut();
    // SAFETY: create an isolated disposable profile, with no capabilities or existing app changes.
    let result = unsafe {
        CreateAppContainerProfile(
            profile_name.as_ptr(),
            profile_name.as_ptr(),
            profile_name.as_ptr(),
            null(),
            0,
            &mut app_sid,
        )
    };
    assert!(result >= 0, "CreateAppContainerProfile: {result:x}");
    struct Profile {
        name: Vec<u16>,
        sid: PSID,
    }
    impl Drop for Profile {
        fn drop(&mut self) {
            // SAFETY: profile and SID are exclusively owned by this disposable test.
            unsafe {
                FreeSid(self.sid);
                DeleteAppContainerProfile(self.name.as_ptr());
            }
        }
    }
    let profile = Profile {
        name: profile_name,
        sid: app_sid,
    };
    let capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: profile.sid,
        ..Default::default()
    };
    let mut bytes = 0;
    // SAFETY: size query, followed by aligned storage of exactly the required attribute capacity.
    unsafe {
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
    }
    let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    let attributes = storage.as_mut_ptr().cast();
    // SAFETY: valid aligned attribute storage and capability SID kept alive until CreateProcessW.
    unsafe {
        assert_ne!(
            InitializeProcThreadAttributeList(attributes, 1, 0, &mut bytes),
            0
        );
        assert_ne!(
            UpdateProcThreadAttribute(
                attributes,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                (&capabilities as *const SECURITY_CAPABILITIES).cast(),
                size_of::<SECURITY_CAPABILITIES>(),
                null_mut(),
                null()
            ),
            0
        );
    }
    let startup = STARTUPINFOEXW {
        StartupInfo: STARTUPINFOW {
            cb: size_of::<STARTUPINFOEXW>() as u32,
            ..Default::default()
        },
        lpAttributeList: attributes,
    };
    let mut command = transport::wide(&format!(
        "\"{}\" --exact appcontainer_child_probe --nocapture",
        probe.display()
    ));
    let mut environment: Vec<String> = std::env::vars()
        .filter(|(name, _)| !name.starts_with("RETYPE_TEST_"))
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    environment.push(format!("RETYPE_TEST_PIPE={}", broker.pipe));
    environment.push(format!(
        "RETYPE_TEST_PRIVATE_DB={}",
        broker.root.join("learning/user.db").display()
    ));
    environment.sort_by_key(|item| item.to_ascii_lowercase());
    let environment = transport::wide(&(environment.join("\0") + "\0"));
    let mut process = PROCESS_INFORMATION::default();
    // SAFETY: all startup/attribute/command/environment buffers remain valid during process creation.
    let created = unsafe {
        CreateProcessW(
            null(),
            command.as_mut_ptr(),
            null(),
            null(),
            0,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            environment.as_ptr().cast(),
            transport::wide(&broker.root.to_string_lossy()).as_ptr(),
            &startup.StartupInfo,
            &mut process,
        )
    };
    let error = std::io::Error::last_os_error();
    // SAFETY: attribute list is no longer needed after the process creation attempt.
    unsafe {
        DeleteProcThreadAttributeList(attributes);
    }
    assert_ne!(created, 0, "AppContainer process launch: {error}");
    let mut exit = 1;
    // SAFETY: exclusively owned process handles; timed-out probes are terminated before cleanup.
    unsafe {
        let wait = WaitForSingleObject(process.hProcess, 15000);
        if wait != WAIT_OBJECT_0 {
            TerminateProcess(process.hProcess, 1);
            WaitForSingleObject(process.hProcess, 1000);
        }
        GetExitCodeProcess(process.hProcess, &mut exit);
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
        assert_eq!(wait, WAIT_OBJECT_0, "AppContainer probe timed out");
    }
    assert_eq!(exit, 0, "AppContainer probe failed");
    let snapshot = broker
        .call(&Broker::poll("after-appcontainer"))
        .unwrap()
        .snapshot
        .unwrap();
    assert_eq!(snapshot.entries[0].text, "你好");
}
