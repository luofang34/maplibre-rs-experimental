use super::*;

#[test]
fn malformed_metadata_is_reported_instead_of_silently_omitted(
) -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::open_in_memory()?;
    connection.execute_batch("CREATE TABLE metadata (name TEXT, value TEXT); INSERT INTO metadata VALUES ('format', NULL);")?;
    let result = extract_metadata(&connection, Path::new("unused-invalid-metadata-output"));
    assert!(matches!(
        result,
        Err(ExtractionError::Sql(rusqlite::Error::InvalidColumnType(..)))
    ));
    Ok(())
}
