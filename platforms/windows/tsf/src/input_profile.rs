//! Numeric-only input diagnostics; writes use the existing nonblocking log queue.
use std::{
    cell::Cell,
    sync::{
        atomic::{AtomicU32, Ordering},
        OnceLock,
    },
    time::{Duration, Instant},
};

static SERIAL: AtomicU32 = AtomicU32::new(1);
thread_local! {
    static LAST_EDIT: Cell<Option<Instant>> = const { Cell::new(None) };
    static LAST_PAINT: Cell<Option<Instant>> = const { Cell::new(None) };
}

fn verbose() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("RETYPE_PROFILE_INPUT").is_some_and(|v| v == "1"))
}

fn should_record(duration: Duration, paint: bool) -> bool {
    if verbose() {
        return true;
    }
    if duration < Duration::from_millis(16) {
        return false;
    }
    let check = |last: &Cell<Option<Instant>>| {
        let now = Instant::now();
        if last
            .get()
            .is_some_and(|previous| now.duration_since(previous) < Duration::from_millis(250))
        {
            return false;
        }
        last.set(Some(now));
        true
    };
    if paint {
        LAST_PAINT.with(check)
    } else {
        LAST_EDIT.with(check)
    }
}

pub(crate) fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

pub(crate) struct Timing {
    requested: Instant,
    started: Instant,
    work: &'static str,
    pub pending: u32,
    pub kernel_us: u64,
    pub write_us: u64,
    pub anchor_us: u64,
    pub measure_us: u64,
    pub layout_us: u64,
    pub ui_us: u64,
    pub raster_us: u64,
    pub notify_us: u64,
    pub notify_result: i32,
    pub getter_calls: u64,
    pub string_calls: u64,
    pub updated_flags: u32,
    pub reused: bool,
    pub dpi: i32,
}

impl Timing {
    pub fn new(requested: Instant, work: &'static str, pending: u32) -> Self {
        Self {
            requested,
            started: Instant::now(),
            work,
            pending,
            kernel_us: 0,
            write_us: 0,
            anchor_us: 0,
            measure_us: 0,
            layout_us: 0,
            ui_us: 0,
            raster_us: 0,
            notify_us: 0,
            notify_result: 0,
            getter_calls: 0,
            string_calls: 0,
            updated_flags: 0,
            reused: false,
            dpi: 0,
        }
    }

    pub fn finish(&self, counts: impl FnOnce() -> (u64, usize, usize), result: i32) {
        let total = self.requested.elapsed();
        if !should_record(total, false) {
            return;
        }
        let (generation, input_len, candidates) = counts();
        crate::settings_log::event(
            "input",
            "edit_timing",
            SERIAL.fetch_add(1, Ordering::Relaxed),
            serde_json::json!({
                "work":self.work, "gen":generation, "input_len":input_len, "candidates":candidates,
                "pending":self.pending, "queue_us":micros(self.started.duration_since(self.requested)),
                "total_us":micros(total), "kernel_us":self.kernel_us, "write_us":self.write_us,
                "anchor_us":self.anchor_us, "measure_us":self.measure_us, "layout_us":self.layout_us,
                "ui_us":self.ui_us, "raster_us":self.raster_us, "bitmap_reused":self.reused, "dpi":self.dpi, "result":result,
                "notify_us":self.notify_us, "getter_calls":self.getter_calls,
                "notify_result":self.notify_result,
                "string_calls":self.string_calls, "updated_flags":self.updated_flags,
            }),
        );
    }
}

pub(crate) fn paint(generation: u64, queued: Instant, started: Instant, dpi: i32) {
    let total = queued.elapsed();
    if !should_record(total, true) {
        return;
    }
    crate::settings_log::event(
        "input",
        "paint_timing",
        SERIAL.fetch_add(1, Ordering::Relaxed),
        serde_json::json!({
            "gen":generation, "dpi":dpi, "queue_us":micros(started.duration_since(queued)),
            "present_us":micros(started.elapsed()), "total_us":micros(total),
        }),
    );
}
