//! What every SQLite-backed repository here needs: opening the pool, running the
//! migrations, and moving instants across the type boundary.
//!
//! Three repositories share one database file, so they shared three copies of this. One
//! copy means the migration set, the journal mode and the nanosecond range cannot drift
//! apart between them.

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use serana_domain::StorageError;

/// Connect and bring the schema up to date.
///
/// Every repository runs the same migration set: they are tables in one file, and whichever
/// opens first must leave the database usable by the rest.
pub(crate) async fn open_pool(
    options: SqliteConnectOptions,
    pool_options: SqlitePoolOptions,
) -> Result<SqlitePool, StorageError> {
    let pool = pool_options.connect_with(options).await.map_err(backend)?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|e| StorageError::Backend(format!("migrations failed: {e}")))?;
    Ok(pool)
}

pub(crate) fn backend(error: impl std::fmt::Display) -> StorageError {
    StorageError::Backend(error.to_string())
}

/// jiff nanoseconds are an `i128`; SQLite integers are 64-bit, which caps what we can store
/// at roughly 1678-2262. jiff itself reaches year 9999, so this conversion is a real
/// boundary, not a formality — an instant past 2262 is refused rather than wrapped.
pub(crate) fn to_nanos(ts: jiff::Timestamp) -> Result<i64, StorageError> {
    i64::try_from(ts.as_nanosecond())
        .map_err(|_| StorageError::Corrupt(format!("timestamp {ts} is outside the storable range")))
}

/// Every `i64` is a valid instant for jiff, so this cannot fail in practice; the `Result`
/// is kept because nothing in sqlx's type mapping proves the column holds what we wrote.
pub(crate) fn from_nanos(nanos: i64) -> Result<jiff::Timestamp, StorageError> {
    jiff::Timestamp::from_nanosecond(i128::from(nanos))
        .map_err(|e| StorageError::Corrupt(format!("stored timestamp {nanos} is invalid: {e}")))
}
