//! T-903's fixed questions and one byte-identical rendering of a classifier
//! input. The implementation is shared with the model adapters as
//! [`ports::prompt`] so Gemini and Jev receive identical bytes (CR-01 1.1);
//! this module keeps the historical `classify::prompt` path working.
pub use ports::prompt::*;
