#![no_main]

use libfuzzer_sys::fuzz_target;

// The full lexer -> parser -> evaluator pipeline must never panic.
fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let source = ucl::SourceFile::new("fuzz.ucl", text);
    let mut sink = ucl::DiagnosticSink::new();
    let tokens = ucl::Lexer::new(&source).tokenize(&mut sink);
    if sink.has_errors() {
        return;
    }
    let ast = ucl::Parser::new(tokens).parse(&mut sink);
    if sink.has_errors() {
        return;
    }
    if let Some(ast) = ast {
        let _value = ucl::Evaluator::new().evaluate(&ast, &source, &mut sink);
    }
});
