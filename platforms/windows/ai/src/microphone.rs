//! Desktop-only capture, outside TSF and AppContainer processes. No callback IO.
#![allow(unsafe_code)]
use crate::voice_service::Session;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use windows_sys::Win32::Media::Audio::*;

#[repr(align(8))]
struct Header(WAVEHDR);
struct Buffer {
    samples: Vec<i16>,
    header: Box<Header>,
}
struct Recorder {
    handle: HWAVEIN,
    buffers: Vec<Buffer>,
    next: usize,
}
impl Drop for Recorder {
    fn drop(&mut self) {
        // SAFETY: reset returns all buffers before they are unprepared and freed.
        unsafe {
            waveInReset(self.handle);
            for b in &mut self.buffers {
                waveInUnprepareHeader(
                    self.handle,
                    &mut b.header.0,
                    std::mem::size_of::<WAVEHDR>() as u32,
                );
            }
            waveInClose(self.handle);
        }
    }
}
impl Recorder {
    fn open(device: u32) -> Result<Self, String> {
        let format = WAVEFORMATEX {
            wFormatTag: 1,
            nChannels: 1,
            nSamplesPerSec: 16000,
            nAvgBytesPerSec: 32000,
            nBlockAlign: 2,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let mut handle = std::ptr::null_mut();
        // SAFETY: owned output handle and live PCM format; no callback is registered.
        if unsafe { waveInOpen(&mut handle, device, &format, 0, 0, 0) } != 0 {
            return Err("无法打开麦克风，请检查系统麦克风权限".into());
        }
        let mut recorder = Self {
            handle,
            buffers: Vec::new(),
            next: 0,
        };
        for _ in 0..4 {
            let mut samples = vec![0i16; 1600];
            let mut header = Box::new(Header(WAVEHDR {
                lpData: samples.as_mut_ptr().cast(),
                dwBufferLength: 3200,
                ..unsafe { std::mem::zeroed() }
            }));
            // SAFETY: heap-pinned header and PCM allocation remain alive through reset.
            if unsafe {
                waveInPrepareHeader(handle, &mut header.0, std::mem::size_of::<WAVEHDR>() as u32)
            } != 0
            {
                return Err("无法准备麦克风缓冲区".into());
            }
            recorder.buffers.push(Buffer { samples, header });
        }
        for b in &mut recorder.buffers {
            if unsafe {
                waveInAddBuffer(
                    handle,
                    &mut b.header.0,
                    std::mem::size_of::<WAVEHDR>() as u32,
                )
            } != 0
            {
                return Err("无法启动录音缓冲区".into());
            }
        }
        if unsafe { waveInStart(handle) } != 0 {
            return Err("无法启动麦克风".into());
        }
        Ok(recorder)
    }
    fn drain(&mut self, session: &Session, requeue: bool) -> Result<(), String> {
        for _ in 0..self.buffers.len() {
            let b = &mut self.buffers[self.next];
            // SAFETY: WinMM updates these flags; volatile access observes completed buffers.
            let flags = unsafe { std::ptr::read_volatile(&raw const b.header.0.dwFlags) };
            if flags & WHDR_DONE == 0 {
                break;
            }
            let length = (unsafe { std::ptr::read_volatile(&raw const b.header.0.dwBytesRecorded) }
                as usize
                / 2)
            .min(b.samples.len());
            if length != 0 {
                if let Err(error) = session.push(b.samples[..length].to_vec()) {
                    if session.is_stopping() {
                        return Ok(());
                    }
                    return Err(error);
                }
            }
            if requeue
                && unsafe {
                    waveInAddBuffer(
                        self.handle,
                        &mut b.header.0,
                        std::mem::size_of::<WAVEHDR>() as u32,
                    )
                } != 0
            {
                return Err("录音设备已断开".into());
            }
            self.next = (self.next + 1) % self.buffers.len();
        }
        Ok(())
    }
}
pub fn start(session: Arc<Session>, device: u32, stop: Arc<AtomicBool>) -> Result<(), String> {
    std::thread::Builder::new()
        .name("retype-microphone".into())
        .spawn(move || {
            let result = (|| {
                let mut recorder = Recorder::open(device)?;
                let began = std::time::Instant::now();
                while !stop.load(Ordering::Acquire)
                    && !session.cancelled.load(Ordering::Acquire)
                    && session.snapshot().phase == crate::voice::Phase::Recording
                    && began.elapsed() < Duration::from_secs(120)
                {
                    recorder.drain(&session, true)?;
                    std::thread::sleep(Duration::from_millis(10));
                }
                unsafe {
                    waveInReset(recorder.handle);
                }
                if !session.cancelled.load(Ordering::Acquire)
                    && session.snapshot().phase == crate::voice::Phase::Recording
                {
                    recorder.drain(&session, false)?;
                    session.stop()?;
                }
                Ok::<_, String>(())
            })();
            if let Err(error) = result {
                session.fail(&error);
            }
        })
        .map(|_| ())
        .map_err(|_| "无法创建麦克风任务".into())
}
