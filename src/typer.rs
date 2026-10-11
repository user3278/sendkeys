//! The background thread that does the typing, so the window stays responsive.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::backend::{self, Backend};

pub struct Job {
    pub text: String,
    /// Time from one key press to the next.
    pub key_delay: Duration,
    /// Extra pause after every Enter, for consoles that are slow to take a line.
    pub line_delay: Duration,
    /// Time to switch to the target window before typing starts.
    pub start_delay: Duration,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Phase {
    Idle,
    Preparing,
    Countdown { left: Duration },
    Typing { done: usize, total: usize },
    Finished {
        typed: usize,
        /// Characters that are not on the active keyboard layout (see `Backend::on_layout`).
        off_layout: Vec<char>,
    },
    Stopped { typed: usize, total: usize },
    Failed { typed: usize, message: String },
}

impl Phase {
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Phase::Preparing | Phase::Countdown { .. } | Phase::Typing { .. }
        )
    }
}

pub struct Typer {
    jobs: Sender<Job>,
    phase: Arc<Mutex<Phase>>,
    cancel: Arc<AtomicBool>,
    backend_name: &'static str,
}

impl Typer {
    /// `on_change` is called from the worker whenever the phase changes.
    pub fn spawn(on_change: impl Fn() + Send + 'static) -> Self {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (name_tx, name_rx) = mpsc::channel();
        let phase = Arc::new(Mutex::new(Phase::Idle));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            phase: phase.clone(),
            cancel: cancel.clone(),
            on_change: Box::new(on_change),
        };
        std::thread::spawn(move || {
            // The back end lives on this thread; some platform handles cannot move.
            let mut backend = backend::create();
            let _ = name_tx.send(backend.name());
            for job in inbox {
                let outcome = worker.run(backend.as_mut(), &job);
                backend.end();
                worker.set(outcome);
            }
        });
        Self {
            jobs,
            phase,
            cancel,
            backend_name: name_rx.recv().unwrap_or("unavailable"),
        }
    }

    pub fn backend_name(&self) -> &'static str {
        self.backend_name
    }

    pub fn phase(&self) -> Phase {
        self.phase.lock().unwrap().clone()
    }

    pub fn start(&self, job: Job) {
        if self.phase().is_busy() {
            return;
        }
        self.cancel.store(false, Ordering::SeqCst);
        *self.phase.lock().unwrap() = Phase::Preparing;
        let _ = self.jobs.send(job);
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

struct Worker {
    phase: Arc<Mutex<Phase>>,
    cancel: Arc<AtomicBool>,
    on_change: Box<dyn Fn() + Send>,
}

impl Worker {
    fn set(&self, phase: Phase) {
        *self.phase.lock().unwrap() = phase;
        (self.on_change)();
    }

    /// Sleeps until `deadline`; returns false if the run was stopped meanwhile,
    /// by the Stop button or by Escape.
    fn wait_until(&self, backend: &mut dyn Backend, deadline: Instant) -> bool {
        loop {
            if self.cancel.load(Ordering::SeqCst) || backend.escape_pressed() {
                return false;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            std::thread::sleep(left.min(Duration::from_millis(20)));
        }
    }

    fn run(&self, backend: &mut dyn Backend, job: &Job) -> Phase {
        let chars = printable(&job.text);
        let total = chars.len();
        let failed = |typed, message| Phase::Failed { typed, message };

        if let Err(message) = backend.begin() {
            return failed(0, message);
        }

        let mut off_layout: Vec<char> = Vec::new();
        for c in &chars {
            if !matches!(c, '\n' | '\t') && !backend.on_layout(*c) && !off_layout.contains(c) {
                off_layout.push(*c);
            }
        }

        let start = Instant::now() + job.start_delay;
        loop {
            let left = start.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            self.set(Phase::Countdown { left });
            if !self.wait_until(backend, Instant::now() + left.min(Duration::from_millis(100))) {
                return Phase::Stopped { typed: 0, total };
            }
        }

        // Keys are held for part of the interval; some consoles miss instant taps.
        let hold = (job.key_delay / 2).min(Duration::from_millis(20));
        let mut next = Instant::now();
        for (done, c) in chars.iter().enumerate() {
            if !self.wait_until(backend, next) {
                return Phase::Stopped { typed: done, total };
            }
            self.set(Phase::Typing { done, total });
            let pressed = Instant::now();
            if let Err(message) = backend.type_char(*c, hold) {
                return failed(done, message);
            }
            next = pressed + job.key_delay;
            if *c == '\n' {
                next += job.line_delay;
            }
        }
        Phase::Finished {
            typed: total,
            off_layout,
        }
    }
}

/// The characters that will be typed: line endings become a single Enter, and
/// control characters other than Tab are dropped.
fn printable(text: &str) -> Vec<char> {
    text.replace("\r\n", "\n")
        .chars()
        .map(|c| if c == '\r' { '\n' } else { c })
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endings_are_normalised_and_controls_dropped() {
        assert_eq!(
            printable("a\r\nb\rc\n\td\u{7}\u{0}é"),
            vec!['a', '\n', 'b', '\n', 'c', '\n', '\t', 'd', 'é']
        );
    }

    struct Recorder {
        typed: String,
        fail_on: Option<char>,
        layout: &'static str,
        escape_after: Option<usize>,
    }

    impl Backend for Recorder {
        fn name(&self) -> &'static str {
            "test"
        }
        fn begin(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn type_char(&mut self, c: char, _hold: Duration) -> Result<(), String> {
            if self.fail_on == Some(c) {
                return Err("boom".into());
            }
            self.typed.push(c);
            Ok(())
        }
        fn on_layout(&self, c: char) -> bool {
            self.layout.is_empty() || self.layout.contains(c)
        }
        fn escape_pressed(&mut self) -> bool {
            self.escape_after == Some(self.typed.len())
        }
        fn end(&mut self) {}
    }

    fn worker() -> Worker {
        Worker {
            phase: Arc::new(Mutex::new(Phase::Idle)),
            cancel: Arc::new(AtomicBool::new(false)),
            on_change: Box::new(|| {}),
        }
    }

    fn job(text: &str, key_delay_ms: u64) -> Job {
        Job {
            text: text.into(),
            key_delay: Duration::from_millis(key_delay_ms),
            line_delay: Duration::ZERO,
            start_delay: Duration::ZERO,
        }
    }

    fn recorder(fail_on: Option<char>) -> Recorder {
        Recorder {
            typed: String::new(),
            fail_on,
            layout: "",
            escape_after: None,
        }
    }

    #[test]
    fn types_everything_in_order() {
        let mut backend = recorder(None);
        let outcome = worker().run(&mut backend, &job("Hi\r\nthere", 0));
        assert_eq!(
            outcome,
            Phase::Finished {
                typed: 8,
                off_layout: vec![]
            }
        );
        assert_eq!(backend.typed, "Hi\nthere");
    }

    #[test]
    fn lists_each_off_layout_character_once() {
        let mut backend = recorder(None);
        backend.layout = "abc ";
        let outcome = worker().run(&mut backend, &job("a é\tb €é\n", 0));
        assert_eq!(
            outcome,
            Phase::Finished {
                typed: 9,
                off_layout: vec!['é', '€']
            }
        );
    }

    #[test]
    fn reports_how_far_it_got_on_failure() {
        let mut backend = recorder(Some('c'));
        let outcome = worker().run(&mut backend, &job("abcd", 0));
        assert_eq!(
            outcome,
            Phase::Failed {
                typed: 2,
                message: "boom".into()
            }
        );
        assert_eq!(backend.typed, "ab");
    }

    #[test]
    fn stop_interrupts_the_countdown_and_the_typing() {
        let w = worker();
        w.cancel.store(true, Ordering::SeqCst);
        let mut backend = recorder(None);
        let mut waiting = job("abc", 0);
        waiting.start_delay = Duration::from_secs(60);
        assert_eq!(
            w.run(&mut backend, &waiting),
            Phase::Stopped { typed: 0, total: 3 }
        );
        assert_eq!(
            w.run(&mut backend, &job("abc", 0)),
            Phase::Stopped { typed: 0, total: 3 }
        );
        assert_eq!(backend.typed, "");
    }

    #[test]
    fn escape_stops_the_typing() {
        let mut backend = recorder(None);
        backend.escape_after = Some(2);
        assert_eq!(
            worker().run(&mut backend, &job("abcd", 0)),
            Phase::Stopped { typed: 2, total: 4 }
        );
        assert_eq!(backend.typed, "ab");
    }

    #[test]
    fn key_delay_paces_the_typing() {
        let mut backend = recorder(None);
        let started = Instant::now();
        worker().run(&mut backend, &job("abcde", 20));
        // Five keys are four gaps of 20 ms.
        assert!(started.elapsed() >= Duration::from_millis(80));
    }
}
