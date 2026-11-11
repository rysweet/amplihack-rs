//! The only GraphQL document exposed by the evidence tool: read-only threads.
pub const THREADS:&str = "query($owner:String!,$repo:String!,$pr:Int!,$cursor:String){repository(owner:$owner,name:$repo){pullRequest(number:$pr){headRefOid reviewThreads(first:100,after:$cursor){nodes{id isResolved isOutdated} pageInfo{hasNextPage endCursor}}}}}";
