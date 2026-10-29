//! Similar artists: candidates from ListenBrainz and Last.fm, ranked by an LLM into edges.

pub mod handlers;
pub mod model;
pub mod rerank;

pub use model::run_job;
