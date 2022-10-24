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

# See also

* https://github.com/vi/postsse for similar application, but for HTTP POST and HTTP GET (SSE) instead of WebSockets.
