//! Built-in post-processors that ship with xberg.
//!
//! Each submodule registers a single [`PostProcessor`](crate::plugins::PostProcessor)
//! implementation behind its own feature gate so non-OSS targets (no-ort-target,
//! wasm-target, android-target) compile out cleanly. Modules added by parallel
//! work streams plug in here without touching one another's files.

#[cfg(feature = "classification")]
pub mod classification;

#[cfg(feature = "classification")]
pub mod chunk_classification;

#[cfg(feature = "summarization")]
pub mod summarization;

#[cfg(feature = "translation")]
pub mod translation;

#[cfg(feature = "captioning")]
pub mod captioning;

#[cfg(feature = "qr-codes")]
pub mod qr;

#[cfg(feature = "ner")]
pub mod ner;

#[cfg(feature = "redaction")]
pub mod redaction;

/// A built-in processor's display name paired with its registration function.
type BuiltinRegistration = (&'static str, fn() -> crate::Result<()>);

/// Attempt every `(name, register)` pair in order, continuing past a failure
/// instead of stopping at the first one.
///
/// #271: `register_builtin` used to `?`-propagate the first registration
/// failure, which meant one broken processor (e.g. `summarization`) silently
/// prevented every processor listed *after* it (`qr`, `ner`, `redaction`) from
/// ever registering, with no diagnostic pointing at the real cause. Each
/// failure is logged individually via `tracing::error!` as it happens, and all
/// failures are aggregated into a single `Err` so the caller still learns that
/// registration was incomplete — but every processor that *can* register does.
fn register_all(entries: &[BuiltinRegistration]) -> crate::Result<()> {
    let mut failures: Vec<String> = Vec::new();

    for (name, register) in entries {
        if let Err(e) = register() {
            tracing::error!(
                "Failed to register built-in post-processor '{name}': {e}. Configuring this \
                 processor will produce no output for its stage until registration succeeds."
            );
            failures.push(format!("{name}: {e}"));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(crate::XbergError::Plugin {
            message: format!(
                "{} of {} built-in post-processor(s) failed to register: {}",
                failures.len(),
                entries.len(),
                failures.join("; ")
            ),
            plugin_name: "builtin".to_string(),
        })
    }
}

/// Register every built-in post-processor enabled by the active feature set.
///
/// This is the single entry point that callers (including
/// `register_default_post_processors`) use to populate the global
/// post-processor registry with the in-tree built-ins. Each submodule's own
/// `register` function is gated by its feature flag so this aggregate stays
/// safe to call on any target.
///
/// Every enabled processor is attempted regardless of whether an earlier one
/// failed (see `register_all`); the returned `Result` reports failures but does
/// not indicate which processors, if any, still registered successfully.
#[allow(
    clippy::vec_init_then_push,
    reason = "every push below is #[cfg]-gated, so the entries cannot be written as one vec![] literal"
)]
pub fn register_builtin() -> crate::Result<()> {
    #[allow(unused_mut, reason = "mutated conditionally per enabled feature below")]
    let mut entries: Vec<BuiltinRegistration> = Vec::new();

    #[cfg(feature = "classification")]
    entries.push(("classification", classification::register));
    #[cfg(feature = "classification")]
    entries.push(("chunk-classification", chunk_classification::register));
    #[cfg(feature = "summarization")]
    entries.push(("summarization", summarization::register));
    #[cfg(feature = "translation")]
    entries.push(("translation", translation::register));
    #[cfg(feature = "captioning")]
    entries.push(("captioning", captioning::register));
    #[cfg(feature = "qr-codes")]
    entries.push(("qr-codes", qr::register));
    #[cfg(feature = "ner")]
    entries.push(("ner", ner::register));
    #[cfg(feature = "redaction")]
    entries.push(("redaction", redaction::register));

    register_all(&entries)
}

#[cfg(test)]
mod tests {
    use super::register_all;

    #[test]
    fn should_attempt_every_registration_even_when_an_earlier_one_fails() {
        static ATTEMPTED: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());

        fn record(name: &'static str) {
            ATTEMPTED.lock().unwrap().push(name);
        }

        fn ok_a() -> crate::Result<()> {
            record("a");
            Ok(())
        }
        fn failing_b() -> crate::Result<()> {
            record("b");
            Err(crate::XbergError::Plugin {
                message: "boom".to_string(),
                plugin_name: "b".to_string(),
            })
        }
        fn ok_c() -> crate::Result<()> {
            record("c");
            Ok(())
        }

        let result = register_all(&[("a", ok_a), ("b", failing_b), ("c", ok_c)]);

        assert!(result.is_err(), "a failure among the entries must be reported");
        // The critical assertion: 'c' (listed after the failing 'b') must still have
        // been attempted. Before the fix, register_builtin used `?` and stopped dead
        // at the first failure, so 'c' was never even called.
        assert_eq!(*ATTEMPTED.lock().unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn should_return_ok_when_every_registration_succeeds() {
        fn ok() -> crate::Result<()> {
            Ok(())
        }
        assert!(register_all(&[("a", ok), ("b", ok)]).is_ok());
    }

    #[test]
    fn should_name_every_failing_processor_in_the_aggregated_error() {
        fn failing() -> crate::Result<()> {
            Err(crate::XbergError::Plugin {
                message: "nope".to_string(),
                plugin_name: "x".to_string(),
            })
        }
        fn ok() -> crate::Result<()> {
            Ok(())
        }

        let err = register_all(&[("first", failing), ("second", ok), ("third", failing)]).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("first"), "message was: {message}");
        assert!(message.contains("third"), "message was: {message}");
        assert!(!message.contains("second"), "message was: {message}");
    }
}
