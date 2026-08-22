#![no_main]

use libfuzzer_sys::fuzz_target;

// The lexer must never panic or produce invalid spans on any input.
fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let source = ucl::SourceFile::new("fuzz.ucl", text);
    let mut sink = ucl::DiagnosticSink::new();
    let _tokens = ucl::Lexer::new(&source).tokenize(&mut sink);
});
