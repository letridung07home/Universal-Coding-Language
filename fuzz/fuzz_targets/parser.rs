#![no_main]

use libfuzzer_sys::fuzz_target;

// The parser must never panic on any token stream.
fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let source = ucl::SourceFile::new("fuzz.ucl", text);
    let mut sink = ucl::DiagnosticSink::new();
    let tokens = ucl::Lexer::new(&source).tokenize(&mut sink);
    let mut parser = ucl::Parser::new(tokens);
    let _ast = parser.parse(&mut sink);
});
