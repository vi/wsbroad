# wsbroad
Simple websocket broadcaster implemented in Rust

Each WebSocket message sent to a particular URL on this websocket server gets delivered to all other WebSocket clients connected to the same URL. By default allows up to 64 URLs. If client is reading incoming messages too slowly, they are getting dropped for this client; no backpressure and no accumulation of messages in memory.

```
$ wsbroad 127.0.0.1:9002
+ 127.0.0.1:57208 -> /baz
New URL: /123
+ 127.0.0.1:57209 -> /baz
+ 127.0.0.1:57211 -> /baz
- 127.0.0.1:57208 -> /baz
- 127.0.0.1:57211 -> /baz
- 127.0.0.1:57209 -> /baz
Expiring URL: /baz
```

For `wss://` server use some Nginx forwarding.

See [Github releases](https://github.com/vi/wsbroad/releases/) for pre-built versions.

# Usage message
<details><summary> wsbroad --help output</summary>

```
wsbroad

ARGS:
    <listen_addr>
      TCP or other socket socket address to bind and listen for incoming WebSocket connections

      Specify `sd-listen` for socket-activated mode, file path for UNIX socket (start abstrat addresses with @).

OPTIONS:
    --unix-listen-unlink
      remove UNIX socket prior to binding to it

    --unix-listen-chmod <mode>
      change filesystem mode of the newly bound UNIX socket to `owner` (006), `group` (066) or `everybody` (666)

    --unix-listen-uid <uid>
      change owner user of the newly bound UNIX socket to this numeric uid

    --unix-listen-gid <uid>
      change owner group of the newly bound UNIX socket to this numeric uid

    --sd-accept-ignore-environment
      ignore environment variables like LISTEN_PID or LISTEN_FDS and unconditionally use file descritor `3` as a socket in
      sd-listen or sd-listen-unix modes

    --tcp-keepalive <ka_triplet>

    --tcp-reuse-port
      try to set SO_REUSEPORT, so that multiple processes can accept connections from the same port in a round-robin fashion.

      Obviously, URL domains would be different based on which instance does the client land.

    --tcp-only-v6
      set socket's IPV6_V6ONLY to true, to avoid receiving IPv4 connections on IPv6 socket

    --tcp-listen-backlog <bl>
      Maximum number of pending unaccepted connections

    --recv-buffer-size <sz>
      Set size of socket receive buffer size

    --send-buffer-size <sz>
      Set size of socket send buffer size in operating system.
      Together with --max-write-buffer-size, it may affect latency when
      messages need to be dropped on overload

    --write-buffer-size <size_bytes>
      The target minimum size of the in-app write buffer to reach before writing the data to the underlying stream.
      The default value is 128 KiB, but wsbroad flushes after sending every message, so this may be unrelevant.

      May be 0. Needs to be less that --max-write-buffer-size.

    --max-write-buffer-size <size_bytes>
      The max size of the in-app write buffer in bytes. Default is 4 MiB.

      This affects how much messages get buffered before droppign
      or slowing down sender begins. Note that --send-buffer-size also affects
      this behaviour.

      Also indirectly affects max message size

    --max-message-size <size_bytes>
      The maximum size of a message. Default is 1 MiB.

    --max-frame-size <size_bytes>
      The maximum size of a single message frame. Default is 1 MiB.

    --accept-unmasked-frames

    --max-urls <num>
      Maximum number of URLs to handle before rejecting the new ones

    --reflexive
      Also send messages back to the sender

    --backpressure
      Slow down senders if there are active receives that are
      unable to take all the messages fast enough.
      Makes messages reliable.

    --backpressure-with-errors
      Similar to --backpressure, but also disconnect senders to an URL if
      we detected that some receiver is abruptly gone.

      Abruptly means we detected an error when trying to send some data to the
      client's socket, not to receive from it.

    --nodelay
      Set TCP_NODELAY to deliver small messages with less latency

    --stochastic-queue <qlen>
      drop messages to slow receivers not in clusters (i.e. multiple dropped messages in a row),
      but with increasing probability based on congestion level.
      Value is maximum additional queue length. The bigger - the more uniformly message going to be
      dropped when overloaded, but the higher there may be latency for message that go though

      Short queue descreases thgouhput.

      Note that other buffers (--max-write-buffer-size and --send-buffer-size) still apply after this queue.

      Unlike other options, the unit is messages, not bytes

    -h, --help
      Prints help information.
```
</details>

# See also

* https://github.com/vi/postsse for similar application, but for HTTP POST and HTTP GET (SSE) instead of WebSockets.
