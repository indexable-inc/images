use snafu::Snafu;

#[derive(Debug, Snafu)]
pub enum Error {
    /// Collapses the dashboard's foreign-boundary failures (TCP bind, Loro
    /// encode) into one observable message.
    #[snafu(display("dashboard error: {message}"), visibility(pub(crate)))]
    Dashboard { message: String },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
