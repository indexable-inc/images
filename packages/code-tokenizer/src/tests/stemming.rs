use super::helpers::tokenize_stemmed;

#[test]
fn stemmed_running_to_run() {
    assert_eq!(tokenize_stemmed("runningTests"), vec!["run", "test"]);
}

#[test]
fn stemmed_calculated_to_calcul() {
    assert_eq!(tokenize_stemmed("calculatedValues"), vec!["calcul", "valu"]);
}

#[test]
fn stemmed_processing_to_process() {
    assert_eq!(tokenize_stemmed("processingData"), vec!["process", "data"]);
}

#[test]
fn stemmed_handlers() {
    assert_eq!(tokenize_stemmed("errorHandlers"), vec!["error", "handler"]);
}

#[test]
fn stemmed_snake_case() {
    assert_eq!(tokenize_stemmed("running_tests"), vec!["run", "test"]);
}

#[test]
fn stemmed_complex_function_name() {
    assert_eq!(
        tokenize_stemmed("calculateUserTotals"),
        vec!["calcul", "user", "total"]
    );
}

#[test]
fn stemmed_preserves_short_words() {
    assert_eq!(
        tokenize_stemmed("getUserById"),
        vec!["get", "user", "by", "id"]
    );
}

#[test]
fn real_world_rust_function() {
    assert_eq!(
        tokenize_stemmed("fn process_incoming_requests() {}"),
        vec!["fn", "process", "incom", "request"]
    );
}

#[test]
fn real_world_class_name() {
    assert_eq!(
        tokenize_stemmed("class DatabaseConnectionManager"),
        vec!["class", "databas", "connect", "manag"]
    );
}

#[test]
fn searching_with_stems() {
    let indexed = tokenize_stemmed("runningTests");
    let query = tokenize_stemmed("run");
    let first_token = query.first().expect("query should have at least one token");
    assert!(indexed.contains(first_token));
}

#[test]
fn searching_handlers_with_handler() {
    let indexed = tokenize_stemmed("errorHandlers");
    let query = tokenize_stemmed("handler");
    let first_token = query.first().expect("query should have at least one token");
    assert!(indexed.contains(first_token));
}

#[test]
fn searching_calculated_with_calculate() {
    let indexed = tokenize_stemmed("calculatedValues");
    let query = tokenize_stemmed("calculate");
    let first_token = query.first().expect("query should have at least one token");
    assert!(indexed.contains(first_token));
}
