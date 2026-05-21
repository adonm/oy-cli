//! Provider integration, model routing, sessions, transcripts, context
//! compaction, and the chat/tool loop orchestration.
//!
//! This module owns provider discovery, model selection, and the session
//! state machine that drives LLM requests through [`crate::llm`].

pub(crate) mod auth;
pub(crate) mod compaction;
pub(crate) mod model;
pub(crate) mod opencode_models;
pub(crate) mod retry;
pub(crate) mod session;
pub(crate) mod transcript;
