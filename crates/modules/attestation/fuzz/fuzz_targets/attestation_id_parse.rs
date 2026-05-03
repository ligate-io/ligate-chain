//! Fuzz `AttestationId::from_str` (the colon-separated
//! `<lsc1...>:<lph1...>` form used in REST paths).
//!
//! On a successful parse, also assert Display round-trips back to
//! the same string. This catches asymmetric parsing bugs where
//! `from_str` accepts inputs that `Display` can't reproduce.

#![no_main]

use core::str::FromStr;

use attestation::AttestationId;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = core::str::from_utf8(data) else {
        return;
    };

    if let Ok(parsed) = AttestationId::from_str(s) {
        // Display(from_str(x)) must equal x for any x that parses
        // cleanly. Otherwise a request with the same canonical id
        // could be accepted twice with different cache keys.
        assert_eq!(parsed.to_string(), s);
    }
});
