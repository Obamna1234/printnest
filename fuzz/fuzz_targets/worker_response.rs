//! Any bytes as an answer from a (possibly compromised) decoder worker:
//! the app-side validation must never panic.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let max = data.first().map(|b| u32::from(*b) + 1);
    let _ = printnest_sandbox::parent::check_response(data, None);
    let _ = printnest_sandbox::parent::check_response(data, max);
});
