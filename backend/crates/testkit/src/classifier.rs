//! A scripted classifier fake.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::Classification;
use ports::{Classifier, ClassifierError, ClassifierId, ClassifierInput};
use sha2::{Digest, Sha256};

/// A scripted classifier.
pub struct FakeClassifier {
    id: ClassifierId,
    script: Mutex<HashMap<String, Result<Classification, ClassifierError>>>,
    default: Mutex<Result<Classification, ClassifierError>>,
    calls: AtomicU64,
    in_flight: AtomicU64,
    max_in_flight: AtomicU64,
    gate: Option<Arc<tokio::sync::Semaphore>>,
    inputs: Mutex<Vec<String>>,
}

impl FakeClassifier {
    pub fn new(id: &str, default: Result<Classification, ClassifierError>) -> Self {
        Self {
            id: ClassifierId(id.to_owned()),
            script: Mutex::new(HashMap::new()),
            default: Mutex::new(default),
            calls: AtomicU64::new(0),
            in_flight: AtomicU64::new(0),
            max_in_flight: AtomicU64::new(0),
            gate: None,
            inputs: Mutex::new(Vec::new()),
        }
    }

    /// Keyed by `input.subject`.
    pub fn script(&self, subject: &str, r: Result<Classification, ClassifierError>) {
        self.script
            .lock()
            .unwrap_or_else(|_| panic!("classifier poisoned"))
            .insert(subject.to_owned(), r);
    }

    pub fn with_gate(self, gate: Arc<tokio::sync::Semaphore>) -> Self {
        Self {
            gate: Some(gate),
            ..self
        }
    }

    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn max_in_flight(&self) -> u64 {
        self.max_in_flight.load(Ordering::SeqCst)
    }

    /// A digest per call, never the text.
    pub fn inputs(&self) -> Vec<String> {
        self.inputs
            .lock()
            .unwrap_or_else(|_| panic!("classifier poisoned"))
            .clone()
    }
}

#[async_trait]
impl Classifier for FakeClassifier {
    fn id(&self) -> ClassifierId {
        self.id.clone()
    }

    async fn classify(&self, input: &ClassifierInput) -> Result<Classification, ClassifierError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let cur = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(cur, Ordering::SeqCst);

        if let Some(gate) = &self.gate {
            let _permit = gate
                .acquire()
                .await
                .map_err(|_| ClassifierError::Unavailable)?;
        }

        let mut hasher = Sha256::new();
        hasher.update(input.subject.as_bytes());
        hasher.update(input.from_domain.as_bytes());
        hasher.update(input.list_id.as_deref().unwrap_or_default().as_bytes());
        let digest = format!("{:x}", hasher.finalize());
        self.inputs
            .lock()
            .unwrap_or_else(|_| panic!("classifier poisoned"))
            .push(digest);

        let result = {
            let script = self
                .script
                .lock()
                .unwrap_or_else(|_| panic!("classifier poisoned"));
            script.get(&input.subject).cloned().unwrap_or_else(|| {
                self.default
                    .lock()
                    .unwrap_or_else(|_| panic!("classifier poisoned"))
                    .clone()
            })
        };

        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        result
    }
}
