//! A scripted `Classifier` for the bake-off pipeline tests (T-901).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use domain::Classification;
use ports::{Classifier, ClassifierError, ClassifierId, ClassifierInput};

/// What a call does.
#[derive(Clone, Debug)]
pub enum Scripted {
    Answer(Classification),
    Error(ClassifierError),
    /// Waits on the (pausable) tokio clock, then does the inner script.
    Delay(Duration, Box<Scripted>),
}

/// A classifier that answers from one script and logs what it received.
pub struct FakeClassifier {
    id: ClassifierId,
    script: Mutex<Scripted>,
    calls: Mutex<Vec<ClassifierInput>>,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}

impl FakeClassifier {
    /// Answers `Disabled` until scripted.
    pub fn new(id: &str) -> Self {
        Self {
            id: ClassifierId(id.to_owned()),
            script: Mutex::new(Scripted::Error(ClassifierError::Disabled)),
            calls: Mutex::new(Vec::new()),
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
        }
    }

    pub fn script_all(&self, s: Scripted) {
        if let Ok(mut guard) = self.script.lock() {
            *guard = s;
        }
    }

    /// What it received, for byte-identity checks.
    pub fn calls(&self) -> Vec<ClassifierInput> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().map_or(0, |c| c.len())
    }

    pub fn max_in_flight(&self) -> usize {
        self.max_in_flight.load(Ordering::SeqCst)
    }
}

async fn run(script: Scripted) -> Result<Classification, ClassifierError> {
    let mut current = script;
    loop {
        match current {
            Scripted::Answer(c) => return Ok(c),
            Scripted::Error(e) => return Err(e),
            Scripted::Delay(d, inner) => {
                tokio::time::sleep(d).await;
                current = *inner;
            }
        }
    }
}

#[async_trait]
impl Classifier for FakeClassifier {
    fn id(&self) -> ClassifierId {
        self.id.clone()
    }

    async fn classify(&self, input: &ClassifierInput) -> Result<Classification, ClassifierError> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(input.clone());
        }
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        let script = self
            .script
            .lock()
            .map_or(Scripted::Error(ClassifierError::Unavailable), |s| s.clone());
        let out = run(script).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        out
    }
}
