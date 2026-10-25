# LAN clipboard

Settings → **Cross-device / 跨设备** enables plain-text clipboard synchronization between Windows, macOS, and Android over a physical LAN or a reachable Tailscale network. This is independent of WebDAV cloud sync and requires no cloud account.

## Connect devices

1. Enable **文字剪贴板同步** on both devices.
2. On either device, select **查看匹配码** to display a temporary six-digit code.
3. On the other device, select **关联设备**, enter that code and select **关联**. Leave the optional address blank for local discovery, or enter the destination computer's IP/MagicDNS hostname and port for Tailscale or routed-subnet connections. The code is single-use, expires after two minutes and can be refreshed or cancelled. Five unsuccessful connection attempts invalidate the pairing window.
4. Copy new text on either device. The other device receives it in its system clipboard. Windows can paste with Ctrl+V; an idle Android keyboard also offers a one-line preview beside its logo, tapped to insert the text.

Android associates with one computer; a desktop can associate with up to eight devices. Android initiates connections; both Windows and macOS can initiate or accept desktop connections. The destination computer must be running retype's sync helper.

## Tailscale and changing networks

- If both devices run Tailscale, use the destination computer's Tailscale IP or MagicDNS name. If it is behind a subnet router, use its routed LAN address instead. Tailscale routes and access rules, plus the destination OS firewall, must allow the sync TCP port.
- Tailscale does not carry the existing mDNS discovery across subnets. Explicit addresses work without discovery; mDNS failure does not disable saved-address reconnection.
- Existing bindings remain valid across network changes. Select **连接地址…** for an already associated device to add a Tailscale address; no new matching code is required. A new address is only a routing hint: the existing device ID and cryptographic credentials must still verify.
- Desktop peers learn advertised endpoints only after authentication. Android retains its previous successful address and can combine it with manually entered addresses. Dialers try physical LAN addresses before numeric Tailscale addresses, try alternatives on failure, and retry with bounded backoff. A healthy connection is retained; a dead connection may take up to the heartbeat timeout before failover begins.
- The default TCP port is **50206**. Existing installations keep their saved port. Desktop settings show actual local addresses and allow changing the listening port. An occupied port is reported instead of silently selecting another random port; update the remote saved address after intentionally changing it.
- IPv4 private/local addresses, Tailscale's exact `100.64.0.0/10` range, and local/ULA IPv6 addresses are supported. Domain names use the system resolver and every resolved destination is checked against the same address policy. IPv6 literals use `[address]:port` syntax; IPv6 listening depends on OS availability.
- Six-digit codes remain single-use identity verification, not network locators. Device identity or credential loss still requires pairing again. No Tailscale account credentials, SSH service, public relay, or router port forwarding are needed for retype's transport.

Guest Wi-Fi isolation or a blocked OS firewall can prevent discovery or connection. Subnet-route approval and access policy are separate requirements. With the default Tailscale subnet SNAT, the remote client can initiate one connection and exchange clipboard text in both directions; no reverse application connection is required.

## Behavior

- Text only, at most 64 KiB of UTF-8, preserving whitespace, line breaks and emoji. Images, files and rich formatting are excluded.
- Only the latest event is retained, in memory, for ten minutes. No clipboard history is written to disk or included in WebDAV snapshots. Existing clipboard contents are not uploaded when enabling the feature.
- A received event is not retransmitted as a new copy. Per-device sequence numbers reject duplicates; a hybrid logical clock and stable device ordering resolve simultaneous copies. Reconnect exchanges unexpired events without replaying history.
- Windows listens to clipboard change notifications in the single user helper. Clipboard contention retries without blocking input hosts. Each slow recipient has one pending text slot, so bursts coalesce to the latest event rather than building an unbounded queue.
- Android reads clipboard changes only while retype is the default input method. Clipboard access can be affected by device policies. Process eviction or sleep can interrupt the connection; opening the keyboard resumes it. This version does not promise continuous synchronization while Android suspends the process.
- Password fields hide keyboard previews; Android sensitivity markers and Windows `ExcludeClipboardContentFromMonitorProcessing` skip outgoing content. Unmarked secrets cannot reliably be recognized.
- Unlinking on the computer revokes the peer's credential and closes its connection. Unlinking on Android removes its local credential; remove the device on the computer too if it should disappear from that computer's list.

## Protocol and storage

The Windows sync helper exposes a TLS TCP endpoint and advertises `_retype-clip._tcp.local.` using DNS-SD. Private/local and Tailscale source addresses only are accepted; there is no public relay or WebDAV clipboard transport. Android pins the computer certificate after pairing.

Protocol version 2 uses SPAKE2 (RustCrypto's Ed25519 implementation) with a random six-digit one-time password. Its identity binds the protocol version, both device IDs and the TLS certificate hash. Role-specific HMAC key-confirmation proofs establish trust before saving the authentication token and pinning the certificate. The code and any code hash are never advertised or sent over the network. Subsequent connections authenticate with the existing token over certificate-pinned TLS. Discovery advertises only identity, version and whether the computer is displaying or entering a code. Discovery information alone does not establish trust.

The UI pairing roles are independent of transport: Android remains the client even when Android displays the code and a desktop enters it. Desktop peers can use either TCP direction. When an old server initiates a connection to its old client, fresh nonce-based HMAC proofs derived from the original shared token authenticate both device IDs and the current TLS certificate before adopting a certificate pin; this does not reveal the token or require another matching code. An existing certificate pin is always checked, and never replaced on mismatch. Both desktops need the updated protocol extension for this reversed-role resume; the original pinned-token connection remains compatible with older peers. Pairing codes and exchanges are ephemeral; on Windows only the displayed code is exposed to the private local status file. The manually entered code is consumed from the private command file and retained in memory for its validity window. The cross-platform cryptographic exchange is shared Rust code, including on Android through JNI. This implementation is a development preview and has not undergone an independent protocol audit.

Frames are bounded newline-delimited JSON. Messages carry a protocol version, origin, source sequence, logical clock and creation time. Oversized, malformed, expired or unauthorized messages are rejected.

Windows metadata/status lives under `%LOCALAPPDATA%\retype\sync\lan` with private user ACLs; certificate keys and peer tokens use Windows Credential Manager. Android uses a separate local preference file and encrypts its token with Android Keystore. Pairing metadata and credentials are excluded from settings/cloud snapshots.

Desktop connection diagnostics are written to `diagnostics.jsonl` in that directory, or `~/Library/Application Support/retype/sync/lan/` on macOS. Events include listener addresses, discovery, DNS/address-policy failures, each connection candidate, pairing success and errors; they exclude clipboard text, matching codes and credentials. The log rotates at 256 KiB with one previous file. `status.json` shows only the latest state, not historical failures. On macOS, if local-network access was granted after the service started but devices remain undiscovered, restart retype before retrying association.

## Validation

Rust tests cover fragmented frames, limits, duplicate/expired/reordered events, simultaneous-copy convergence, TLS pairing, bidirectional text delivery, reconnection and revocation. The opt-in `android_emulator_bidirectional_clipboard` fixture plus `LanClipboardTest` checks the actual Android TLS client, matching-code exchange, system clipboard and outgoing clipboard listener against a synthetic Windows endpoint without touching the desktop user's clipboard.

Physical Android device acceptance verified Wi-Fi discovery, matching-code association and automatic reconnection to the Windows helper. Clipboard delivery in both directions was verified with the Android emulator; physical-device clipboard delivery and behavior on other Wi-Fi/firewall configurations remain separate acceptance checks.

Address-policy tests cover exact CGNAT boundaries, DNS names, IPv6, invalid ports and URL-component rejection. Desktop integration tests cover pairing by explicit address, unreachable-candidate fallback, certificate mismatch rejection, and reusing an existing binding with reversed TCP roles. The Android synthetic-endpoint test adds a failing candidate and reconnects while asserting that the original peer ID, certificate pin and token remain unchanged.
