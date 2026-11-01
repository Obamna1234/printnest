//! Any bytes as a request to the worker (in-process, without lockdown):
//! the worker's request parsing and decoding must never panic.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut input = data;
    let mut output = Vec::new();
    let _ = printnest_sandbox::worker::serve(&mut input, &mut output);
});
