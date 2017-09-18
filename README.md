# wsbroad
Simple websocket broadcaster implemented in Rust

Each WebSocket message sent to a particular URL on this websocket server gets delivered to all other WebSocket clients connected to the same URL. By default allows up to 64 URLs. If client is reading incoming messages too slowly, they are getting dropped for this client; no backpressure and no accumulation of messages in memory.
