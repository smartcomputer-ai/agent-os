use aos_air_types::HashRef;
use aos_effects::builtins::{HostBlobOutput, HostInlineText, HostOutput, HostTextOutput};
use aos_store::Store;

pub(crate) const DEFAULT_INLINE_OUTPUT_LIMIT_BYTES: usize = 16 * 1024;
pub(crate) const OUTPUT_PREVIEW_BYTES: usize = 512;

#[derive(Clone, Copy)]
pub(crate) struct OutputConfig {
    pub(crate) inline_limit_bytes: usize,
    pub(crate) preview_bytes: usize,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            inline_limit_bytes: DEFAULT_INLINE_OUTPUT_LIMIT_BYTES,
            preview_bytes: OUTPUT_PREVIEW_BYTES,
        }
    }
}

pub(crate) enum OutputMaterializeError {
    InlineRequiredTooLarge(usize),
    Store(String),
}

pub(crate) fn materialize_output<S: Store>(
    store: &S,
    mode: &str,
    bytes: &[u8],
    cfg: OutputConfig,
) -> Result<Option<HostOutput>, OutputMaterializeError> {
    if bytes.is_empty() {
        return Ok(None);
    }

    if mode == "require_inline" {
        if bytes.len() > cfg.inline_limit_bytes {
            return Err(OutputMaterializeError::InlineRequiredTooLarge(bytes.len()));
        }
        return Ok(Some(to_inline_output(bytes)));
    }

    if bytes.len() <= cfg.inline_limit_bytes {
        return Ok(Some(to_inline_output(bytes)));
    }

    let blob = to_blob_output(store, bytes, cfg)?;
    Ok(Some(HostOutput::Blob { blob }))
}

pub(crate) fn materialize_binary_output<S: Store>(
    store: &S,
    mode: &str,
    bytes: &[u8],
    cfg: OutputConfig,
) -> Result<Option<HostOutput>, OutputMaterializeError> {
    if bytes.is_empty() {
        return Ok(None);
    }

    if mode == "require_inline" {
        if bytes.len() > cfg.inline_limit_bytes {
            return Err(OutputMaterializeError::InlineRequiredTooLarge(bytes.len()));
        }
        return Ok(Some(HostOutput::InlineBytes {
            inline_bytes: aos_effects::builtins::HostInlineBytes {
                bytes: bytes.to_vec(),
            },
        }));
    }

    if bytes.len() <= cfg.inline_limit_bytes {
        return Ok(Some(HostOutput::InlineBytes {
            inline_bytes: aos_effects::builtins::HostInlineBytes {
                bytes: bytes.to_vec(),
            },
        }));
    }

    let blob = to_blob_output(store, bytes, cfg)?;
    Ok(Some(HostOutput::Blob { blob }))
}

pub(crate) fn materialize_text_output<S: Store>(
    store: &S,
    mode: &str,
    text: &str,
    cfg: OutputConfig,
) -> Result<Option<HostTextOutput>, OutputMaterializeError> {
    if text.is_empty() {
        return Ok(None);
    }
    let bytes = text.as_bytes();
    if mode == "require_inline" {
        if bytes.len() > cfg.inline_limit_bytes {
            return Err(OutputMaterializeError::InlineRequiredTooLarge(bytes.len()));
        }
        return Ok(Some(HostTextOutput::InlineText {
            inline_text: HostInlineText {
                text: text.to_string(),
            },
        }));
    }

    if bytes.len() <= cfg.inline_limit_bytes {
        return Ok(Some(HostTextOutput::InlineText {
            inline_text: HostInlineText {
                text: text.to_string(),
            },
        }));
    }

    let blob = to_blob_output(store, bytes, cfg)?;
    Ok(Some(HostTextOutput::Blob { blob }))
}

pub(crate) fn output_mode_valid(mode: &str) -> bool {
    mode == "auto" || mode == "require_inline"
}

fn to_inline_output(bytes: &[u8]) -> HostOutput {
    match std::str::from_utf8(bytes) {
        Ok(text) => HostOutput::InlineText {
            inline_text: aos_effects::builtins::HostInlineText {
                text: text.to_string(),
            },
        },
        Err(_) => HostOutput::InlineBytes {
            inline_bytes: aos_effects::builtins::HostInlineBytes {
                bytes: bytes.to_vec(),
            },
        },
    }
}

fn to_blob_output<S: Store>(
    store: &S,
    bytes: &[u8],
    cfg: OutputConfig,
) -> Result<HostBlobOutput, OutputMaterializeError> {
    let hash = store
        .put_blob(bytes)
        .map_err(|err| OutputMaterializeError::Store(err.to_string()))?;
    let blob_ref = HashRef::new(hash.to_hex())
        .map_err(|err| OutputMaterializeError::Store(err.to_string()))?;
    let preview = bytes[..bytes.len().min(cfg.preview_bytes)].to_vec();
    Ok(HostBlobOutput {
        blob_ref,
        size_bytes: bytes.len() as u64,
        preview_bytes: Some(preview),
    })
}
