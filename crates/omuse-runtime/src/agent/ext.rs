//! Adapters to modules the agent calls but that are ported by others: `imagegen` (`ig`), `attachments` (`at`) and
//! `app/common/calc.py` (`calc`). The agent only calls these wrappers, so reconciling with the real modules is a
//! change in this one file.
//!
//! Until the real module lands, each wrapper is the smallest working stand-in (see the module docs); the call shapes
//! mirror the Python functions.

use serde_json::Value;

/// `app/runtime/imagegen.py` as the agent uses it (thin wrapper over [`crate::imagegen::ImageClient`]).
pub mod ig {
    use serde_json::Value;

    pub use crate::imagegen::{EditOpts, GenerateOpts as GenOpts, ImageClient, ImageResult};

    /// `ImageError` (shown to the model through `_img_error`) or any other failure (`Other`).
    #[derive(Debug, Clone)]
    pub enum IgError {
        Image(String),
        Other(String),
    }

    impl From<crate::imagegen::ImageError> for IgError {
        fn from(e: crate::imagegen::ImageError) -> Self {
            IgError::Image(e.0)
        }
    }

    /// `with_style(prompt, style)`.
    pub fn with_style(prompt: &str, style: &Value) -> String {
        crate::imagegen::with_style(prompt, style.as_str())
    }

    pub async fn generate(
        c: &ImageClient,
        settings: &Value,
        prompt: &str,
        o: &GenOpts,
    ) -> Result<ImageResult, IgError> {
        Ok(c.generate(settings, prompt, o).await?)
    }

    pub async fn edit(
        c: &ImageClient,
        settings: &Value,
        images: &[(String, Vec<u8>)],
        prompt: &str,
        o: &EditOpts,
    ) -> Result<ImageResult, IgError> {
        Ok(c.edit(settings, images, prompt, o).await?)
    }

    pub async fn variation(
        c: &ImageClient,
        settings: &Value,
        image: &(String, Vec<u8>),
        o: &EditOpts,
    ) -> Result<ImageResult, IgError> {
        Ok(c.variation(settings, image, o).await?)
    }
}

/// `app/runtime/attachments.py` as the agent uses it (the real module; errors flattened to their message).
pub mod at {
    use std::path::Path;

    use crate::attachments as A;

    /// `AttachmentError` (or any other read failure) with its message.
    #[derive(Debug, Clone)]
    pub struct AttachmentError(pub String);

    impl std::fmt::Display for AttachmentError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }

    fn e(err: anyhow::Error) -> AttachmentError {
        match err.downcast_ref::<A::AttachmentError>() {
            Some(a) => AttachmentError(a.0.clone()),
            None => AttachmentError(err.to_string()),
        }
    }

    pub fn kind_of(path: &str) -> &'static str {
        A::kind_of(path)
    }
    pub fn text_of(path: &Path, limit: usize) -> Result<String, AttachmentError> {
        A::text_of(path, limit).map_err(e)
    }
    pub fn image_b64(path: &Path) -> Result<(String, String), AttachmentError> {
        A::image_b64(path, 1280).map_err(e)
    }
    pub fn video_frames(path: &Path, n: usize) -> Result<Vec<(f64, Vec<u8>)>, AttachmentError> {
        A::video_frames(path, n, 640).map_err(e)
    }
    pub fn contact_sheet(frames: &[(f64, Vec<u8>)]) -> Result<(Vec<u8>, String), AttachmentError> {
        A::contact_sheet(frames, 3, 420).map_err(e)
    }
    pub fn pdf_page_images(path: &Path, max_pages: usize) -> Result<Vec<Vec<u8>>, AttachmentError> {
        A::pdf_page_images(path, max_pages).map_err(e)
    }
    pub fn to_jpeg(data: Vec<u8>, max_side: u32) -> Result<(Vec<u8>, String), AttachmentError> {
        A::to_jpeg(&data, max_side).map_err(e)
    }
    pub fn audio_wav(path: &Path) -> Result<Vec<u8>, AttachmentError> {
        A::audio_wav(path, 600).map_err(e)
    }
}

/// `str(v)` helper re-exported for the adapters' callers.
pub fn s(v: &Value) -> String {
    omuse_common::util::py_str(v)
}
