//! The shared model prompt. It lives in `ports` (see `ports::prompt`) so the
//! API and both model adapters render byte-identical text and ask identical
//! questions; this module re-exports it for existing callers.

pub use ports::prompt::{
    render_model_text, BULK_QUESTION, CLASS_OPTIONS, CLASS_QUESTION, QUESTION_VERSION,
};
