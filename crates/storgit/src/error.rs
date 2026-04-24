#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Git(Box<dyn std::error::Error + Send + Sync + 'static>),
    /// A [`crate::ModuleFetcher`] returned an error.
    Fetch(Box<dyn std::error::Error + Send + Sync + 'static>),
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Git(e) => write!(f, "git: {e}"),
            Error::Fetch(e) => write!(f, "fetch: {e}"),
            Error::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

macro_rules! boxed_from {
    ($($t:ty),+ $(,)?) => {
        $(
            impl From<$t> for Error {
                fn from(e: $t) -> Self { Error::Git(Box::new(e)) }
            }
        )+
    };
}

boxed_from!(
    gix::init::Error,
    gix::open::Error,
    gix::object::find::existing::Error,
    gix::object::write::Error,
    gix::object::commit::Error,
    gix::objs::decode::Error,
    gix::reference::edit::Error,
    gix::revision::walk::Error,
    gix::revision::walk::iter::Error,
    gix::traverse::commit::simple::Error,
);
