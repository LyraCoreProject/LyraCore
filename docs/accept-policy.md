# Accept-loop resource policy

`gateway/src/accept.rs` classifies listener failures and bounds concurrent blocking work.
An error from one accepted connection must leave the listener available for later connections.

`classify_accept_error` treats four raw errnos as fatal:

| errno | listener failure |
| --- | --- |
| `EBADF` | the descriptor is closed |
| `ENOTSOCK` | the descriptor is not a socket |
| `EINVAL` | the socket is not listening or its arguments are invalid |
| `EFAULT` | the address argument is not writable |

These conditions make retrying the same listener pointless. All other errors are transient,
including unknown errnos and errors without a raw errno. The caller logs each transient error.
`AcceptBackoff` caps repeated failures at one attempt per second.

Linux includes `EOPNOTSUPP` among pending network errors that `accept` should retry. Its other meaning,
a non-stream socket, cannot apply to a `TcpListener` created by `bind`. `EPERM` can report a firewall
refusal for one connection and also retries. `EMFILE` reports descriptor exhaustion and must leave
existing sessions alive while capacity recovers.

A shared non-waiting capacity limit keeps accepted sockets out of Tokio's unbounded blocking-task
queue. Holding the permit for the connection's work makes the limit cover the resource it protects.
