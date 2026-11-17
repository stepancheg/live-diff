/// Full commit node id (the hex digest).
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct CommitDigest(pub(crate) String);
