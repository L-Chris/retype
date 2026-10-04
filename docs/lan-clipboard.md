# LAN clipboard

Settings → **Cross-device / 跨设备** enables plain-text clipboard synchronization between Windows and Android on a local network. This is independent of WebDAV cloud sync and requires no cloud account.

## Connect devices

1. Enable **文字剪贴板同步** on both devices.
2. On either device, select **查看匹配码** to display a temporary six-digit code.
3. On the other device, select **关联设备**, enter that code and select **关联**. Local discovery and code verification complete the association automatically, with no address entry or second confirmation. The code is single-use, expires after two minutes and can be refreshed or cancelled. Five unsuccessful connection attempts invalidate the pairing window.
4. Copy new text on either device. The other device receives it in its system clipboard. Windows can paste with Ctrl+V; an idle Android keyboard also offers a one-line preview beside its logo, tapped to insert the text.

The Android client currently associates with one computer; a computer can associate with up to eight mobile devices. Both devices must be reachable over their local network. Guest Wi-Fi isolation or a blocked Windows firewall can prevent discovery or connection. Allow the sync helper on the Windows private network profile when prompted. No router port forwarding is needed.

## Behavior

- Text only, at most 64 KiB of UTF-8, preserving whitespace, line breaks and emoji. Images, files and rich formatting are excluded.
- Only the latest event is retained, in memory, for ten minutes. No clipboard history is written to disk or included in WebDAV snapshots. Existing clipboard contents are not uploaded when enabling the feature.
- A received event is not retransmitted as a new copy. Per-device sequence numbers reject duplicates; a hybrid logical clock and stable device ordering resolve simultaneous copies. Reconnect exchanges unexpired events without replaying history.
- Windows listens to clipboard change notifications in the single user helper. Clipboard contention retries without blocking input hosts. Each slow recipient has one pending text slot, so bursts coalesce to the latest event rather than building an unbounded queue.
- Android reads clipboard changes only while retype is the default input method. Clipboard access can be affected by device policies. Process eviction or sleep can interrupt the connection; opening the keyboard resumes it. This version does not promise continuous synchronization while Android suspends the process.
- Password fields hide keyboard previews; Android sensitivity markers and Windows `ExcludeClipboardContentFromMonitorProcessing` skip outgoing content. Unmarked secrets cannot reliably be recognized.
- Unlinking on the computer revokes the peer's credential and closes its connection. Unlinking on Android removes its local credential; remove the device on the computer too if it should disappear from that computer's list.

## Protocol and storage

The Windows sync helper exposes a TLS TCP endpoint and advertises `_retype-clip._tcp.local.` using DNS-SD. Private/local source addresses only are accepted; there is no public relay or WebDAV clipboard transport. Android pins the computer certificate after pairing.

Protocol version 2 uses SPAKE2 (RustCrypto's Ed25519 implementation) with a random six-digit one-time password. Its identity binds the protocol version, both device IDs and the TLS certificate hash. Role-specific HMAC key-confirmation proofs establish trust before saving the authentication token and pinning the certificate. The code and any code hash are never advertised or sent over the network. Subsequent connections authenticate with the existing token over certificate-pinned TLS. Discovery advertises only identity, version and whether the computer is displaying or entering a code. Discovery information alone does not establish trust.

The UI pairing roles are independent of transport: Windows remains the TLS server and Android the client even when Android displays the code and Windows enters it. Pairing codes and exchanges are ephemeral; on Windows only the displayed code is exposed to the private local status file. The manually entered code is consumed from the private command file and retained in memory for its validity window. The cross-platform cryptographic exchange is shared Rust code, including on Android through JNI. This implementation is a development preview and has not undergone an independent protocol audit.

Frames are bounded newline-delimited JSON. Messages carry a protocol version, origin, source sequence, logical clock and creation time. Oversized, malformed, expired or unauthorized messages are rejected.

Windows metadata/status lives under `%LOCALAPPDATA%\retype\sync\lan` with private user ACLs; certificate keys and peer tokens use Windows Credential Manager. Android uses a separate local preference file and encrypts its token with Android Keystore. Pairing metadata and credentials are excluded from settings/cloud snapshots.

## Validation

Rust tests cover fragmented frames, limits, duplicate/expired/reordered events, simultaneous-copy convergence, TLS pairing, bidirectional text delivery, reconnection and revocation. The opt-in `android_emulator_bidirectional_clipboard` fixture plus `LanClipboardTest` checks the actual Android TLS client, matching-code exchange, system clipboard and outgoing clipboard listener against a synthetic Windows endpoint without touching the desktop user's clipboard.

Physical Android device acceptance verified Wi-Fi discovery, matching-code association and automatic reconnection to the Windows helper. Clipboard delivery in both directions was verified with the Android emulator; physical-device clipboard delivery and behavior on other Wi-Fi/firewall configurations remain separate acceptance checks.
