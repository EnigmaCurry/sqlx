use crate::database::{Database, HasStatementCache};
use crate::error::Error;
use crate::transaction::Transaction;
use futures_core::future::BoxFuture;
use futures_core::Future;
use std::fmt::Debug;
use std::str::FromStr;

/// Represents a single database connection.
pub trait Connection: Send {
    type Database: Database;

    type Options: ConnectOptions<Connection = Self>;

    /// Explicitly close this database connection.
    ///
    /// This method is **not required** for safe and consistent operation. However, it is
    /// recommended to call it instead of letting a connection `drop` as the database backend
    /// will be faster at cleaning up resources.
    fn close(self) -> BoxFuture<'static, Result<(), Error>>;

    /// Checks if a connection to the database is still valid.
    fn ping(&mut self) -> BoxFuture<'_, Result<(), Error>>;

    /// Begin a new transaction or establish a savepoint within the active transaction.
    ///
    /// Returns a [`Transaction`] for controlling and tracking the new transaction.
    fn begin(&mut self) -> BoxFuture<'_, Result<Transaction<'_, Self::Database>, Error>>
    where
        Self: Sized;

    /// Execute the function inside a transaction.
    ///
    /// If the function returns an error, the transaction will be rolled back. If it does not
    /// return an error, the transaction will be committed.
    fn transaction<'c: 'f, 'f, T, E, F, Fut>(&'c mut self, f: F) -> BoxFuture<'f, Result<T, E>>
    where
        Self: Sized,
        T: Send,
        F: FnOnce(&mut <Self::Database as Database>::Connection) -> Fut + Send + 'f,
        E: From<Error> + Send,
        Fut: Future<Output = Result<T, E>> + Send,
    {
        Box::pin(async move {
            let mut tx = self.begin().await?;

            match f(&mut tx).await {
                Ok(r) => {
                    // no error occurred, commit the transaction
                    tx.commit().await?;

                    Ok(r)
                }

                Err(e) => {
                    // an error occurred, rollback the transaction
                    tx.rollback().await?;

                    Err(e)
                }
            }
        })
    }

    /// The number of statements currently cached in the connection.
    fn cached_statements_size(&self) -> usize
    where
        Self::Database: HasStatementCache,
    {
        0
    }

    /// Removes all statements from the cache, closing them on the server if
    /// needed.
    fn clear_cached_statements(&mut self) -> BoxFuture<'_, Result<(), Error>>
    where
        Self::Database: HasStatementCache,
    {
        Box::pin(async move { Ok(()) })
    }

    #[doc(hidden)]
    fn flush(&mut self) -> BoxFuture<'_, Result<(), Error>>;

    #[doc(hidden)]
    fn should_flush(&self) -> bool;

    #[doc(hidden)]
    fn set_has_cancellation(&mut self, has_cancellation: bool);

    /// If this connection previously had a canceled execution future. If true, the connection
    /// should be closed as it may be in an inconsistent state.
    #[doc(hidden)]
    fn has_cancellation(&self) -> bool;

    #[doc(hidden)]
    #[must_use = "don't forget to call `.forget()`"]
    fn cancellation_guard(&mut self) -> CancellationGuard<'_, Self> where Self: Sized {
        self.set_has_cancellation(false);
        CancellationGuard { conn: self, ignore: false }
    }

    /// Establish a new database connection.
    ///
    /// A value of `Options` is parsed from the provided connection string. This parsing
    /// is database-specific.
    #[inline]
    fn connect(url: &str) -> BoxFuture<'static, Result<Self, Error>>
    where
        Self: Sized,
    {
        let options = url.parse();

        Box::pin(async move { Ok(Self::connect_with(&options?).await?) })
    }

    /// Establish a new database connection with the provided options.
    fn connect_with(options: &Self::Options) -> BoxFuture<'_, Result<Self, Error>>
    where
        Self: Sized,
    {
        options.connect()
    }
}

pub trait ConnectOptions: 'static + Send + Sync + FromStr<Err = Error> + Debug {
    type Connection: Connection + ?Sized;

    /// Establish a new database connection with the options specified by `self`.
    fn connect(&self) -> BoxFuture<'_, Result<Self::Connection, Error>>
    where
        Self::Connection: Sized;
}

pub struct CancellationGuard<'a, C: Connection> {
    pub conn: &'a mut C,
    pub ignore: bool
}

impl<'a, C: Connection> Drop for CancellationGuard<'a, C> {
    fn drop(&mut self) {
        if !self.ignore {
            self.conn.set_has_cancellation(true);
        }
    }
}
