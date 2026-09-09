import 'dart:async';
import 'dart:io';
import 'package:flutter/foundation.dart';

/// Low-latency TCP socket client for transmitting binary input events from mobile device to host daemon.
class InputSocketClient {
  /// Delay before the first reconnection attempt after a drop.
  static const Duration initialRetryDelay = Duration(milliseconds: 250);

  /// Ceiling the retry delay backs off to, so a host that stays down is not hammered.
  static const Duration maxRetryDelay = Duration(seconds: 5);

  Socket? _socket;
  StreamSubscription<Uint8List>? _subscription;
  Timer? _reconnectTimer;
  Duration _retryDelay = initialRetryDelay;
  bool _isConnecting = false;
  bool _isConnected = false;

  /// Set by [disconnect] so a deliberate teardown is not immediately undone by the retry
  /// loop, and cleared by the next explicit [connect].
  bool _closedByCaller = false;

  String _host = '127.0.0.1';
  int _port = 6001;

  /// Returns `true` if the input TCP socket is connected to the host server.
  bool get isConnected => _isConnected;

  /// Serializes an input event into a 20-byte binary packet payload.
  ///
  /// ### Packet Binary Layout (20 Bytes Little-Endian)
  /// - `[0]`: Magic Byte (`0x53` = 'S')
  /// - `[1]`: Event Type Identifier Code (`eventType`)
  /// - `[2..4]`: Finger Slot ID or Flags (`slotOrFlags` uint16)
  /// - `[4..8]`: Parameter 1 (`param1` float32)
  /// - `[8..12]`: Parameter 2 (`param2` float32)
  /// - `[12..16]`: Parameter 3 (`param3` float32)
  /// - `[16..20]`: Parameter 4 (`param4` float32)
  static Uint8List buildBinaryPacket({
    required int eventType,
    int slotOrFlags = 0,
    double param1 = 0.0,
    double param2 = 0.0,
    double param3 = 0.0,
    double param4 = 0.0,
  }) {
    final buffer = Uint8List(20);
    final byteData = ByteData.sublistView(buffer);
    byteData.setUint8(0, 0x53); // Magic Byte 'S'
    byteData.setUint8(1, eventType);
    byteData.setUint16(2, slotOrFlags, Endian.little);
    byteData.setFloat32(4, param1, Endian.little);
    byteData.setFloat32(8, param2, Endian.little);
    byteData.setFloat32(12, param3, Endian.little);
    byteData.setFloat32(16, param4, Endian.little);
    return buffer;
  }

  /// Establishes an asynchronous TCP connection to the host input server, and keeps it up:
  /// if the connection later drops, it is retried with exponential backoff until [disconnect]
  /// is called.
  Future<void> connect({String host = '127.0.0.1', int port = 6001}) async {
    _host = host;
    _port = port;
    _closedByCaller = false;
    if (_isConnected || _isConnecting) return;

    _isConnecting = true;
    _reconnectTimer?.cancel();
    _reconnectTimer = null;

    try {
      debugPrint('Connecting Input Socket to $host:$port...');
      final socket = await Socket.connect(
        host,
        port,
        timeout: const Duration(seconds: 5),
      );

      // disconnect() may have been called while the handshake was in flight.
      if (_closedByCaller) {
        socket.destroy();
        return;
      }

      socket.setOption(SocketOption.tcpNoDelay, true);
      _socket = socket;
      _isConnected = true;
      _retryDelay = initialRetryDelay;

      // The host never sends anything back on this socket, but the stream still has to be
      // listened to. Without a subscription, a peer that goes away is never noticed: `add()`
      // on a dead socket does not throw synchronously, it reports through this stream, so the
      // failure surfaced as an unhandled exception while `_isConnected` stayed true forever
      // and every later event was written into the void.
      _subscription = socket.listen(
        (_) {},
        onDone: () => _handleDrop('closed by host'),
        onError: (Object e) => _handleDrop('socket error: $e'),
        cancelOnError: true,
      );

      debugPrint('SUCCESS: Input Socket Connected to $host:$port');
    } catch (e) {
      debugPrint('Input Socket connection failed ($host:$port): $e');
      _isConnected = false;
      _scheduleReconnect();
    } finally {
      _isConnecting = false;
    }
  }

  /// Transmits a touchscreen contact event ('DOWN', 'MOVE', or 'UP') with normalized coordinates.
  void sendTouchEvent(
    String action,
    double xNorm,
    double yNorm,
    int pointerId,
  ) {
    int eventType;
    switch (action) {
      case 'DOWN':
        eventType = 0x01;
        break;
      case 'MOVE':
        eventType = 0x02;
        break;
      case 'UP':
        eventType = 0x03;
        break;
      default:
        return;
    }
    final pkt = buildBinaryPacket(
      eventType: eventType,
      slotOrFlags: pointerId,
      param1: xNorm,
      param2: yNorm,
    );
    _sendPacket(pkt);
  }

  /// Transmits an active stylus event ('DOWN', 'MOVE', or 'UP') with pressure and tilt properties.
  void sendStylusEvent(
    String action,
    double xNorm,
    double yNorm,
    double pressure, {
    double tiltX = 0.0,
    double tiltY = 0.0,
  }) {
    int eventType;
    switch (action) {
      case 'DOWN':
        eventType = 0x04;
        break;
      case 'MOVE':
        eventType = 0x05;
        break;
      case 'UP':
        eventType = 0x06;
        break;
      default:
        return;
    }
    final pkt = buildBinaryPacket(
      eventType: eventType,
      param1: xNorm,
      param2: yNorm,
      param3: pressure,
      param4: tiltX,
    );
    _sendPacket(pkt);
  }

  /// Transmits a 2-axis page scrolling delta event (`dx`, `dy`).
  void sendScrollEvent(double dx, double dy) {
    final pkt = buildBinaryPacket(eventType: 0x07, param1: dx, param2: dy);
    _sendPacket(pkt);
  }

  /// Transmits a GNOME Overview toggle gesture event.
  void sendOverviewGesture() {
    final pkt = buildBinaryPacket(eventType: 0x09);
    _sendPacket(pkt);
  }

  /// Transmits a continuous 1:1 swipe gesture delta event (`dx`, `dy`).
  void sendSwipeGesture(double dx, double dy) {
    final pkt = buildBinaryPacket(eventType: 0x0B, param1: dx, param2: dy);
    _sendPacket(pkt);
  }

  /// Transmits a contextual right-click event at normalized screen coordinates.
  void sendRightClickEvent(double xNorm, double yNorm) {
    final pkt = buildBinaryPacket(
      eventType: 0x08,
      param1: xNorm,
      param2: yNorm,
    );
    _sendPacket(pkt);
  }

  /// Transmits an initial resolution handshake event communicating client screen dimensions.
  void sendInitResolution(int width, int height) {
    if (width <= 0 || height <= 0) return;
    final pkt = buildBinaryPacket(
      eventType: 0x0A,
      param1: width.toDouble(),
      param2: height.toDouble(),
    );
    _sendPacket(pkt);
  }

  /// Sends a raw 20-byte binary packet payload over the connected socket.
  void _sendPacket(Uint8List packet) {
    final socket = _socket;
    if (!_isConnected || socket == null) {
      // Dropped deliberately. Input only means anything live, and the retry loop already owns
      // getting the connection back - reconnecting from here would fire one attempt per event
      // during a touch burst.
      _scheduleReconnect();
      return;
    }
    try {
      socket.add(packet);
    } catch (e) {
      _handleDrop('write failed: $e');
    }
  }

  /// Marks the connection lost and queues a reconnection attempt.
  void _handleDrop(String reason) {
    if (!_isConnected && _socket == null) return;
    debugPrint('Input socket dropped ($reason)');
    _teardownSocket();
    _scheduleReconnect();
  }

  /// Cancels the stream subscription and destroys the socket, leaving retry state alone.
  void _teardownSocket() {
    _isConnected = false;
    _subscription?.cancel();
    _subscription = null;
    try {
      _socket?.destroy();
    } catch (e) {
      debugPrint('Error closing socket: $e');
    }
    _socket = null;
  }

  /// Queues one reconnection attempt, backing off exponentially up to [maxRetryDelay].
  /// Does nothing after an explicit [disconnect], or while an attempt is already queued.
  void _scheduleReconnect() {
    if (_closedByCaller || _reconnectTimer != null || _isConnecting) return;

    final delay = _retryDelay;
    debugPrint('Reconnecting input socket in ${delay.inMilliseconds} ms');
    _reconnectTimer = Timer(delay, () {
      _reconnectTimer = null;
      connect(host: _host, port: _port);
    });

    final next = delay * 2;
    _retryDelay = next > maxRetryDelay ? maxRetryDelay : next;
  }

  /// Closes the input socket connection, cancels any queued reconnection, and cleans up
  /// resources. The connection stays down until [connect] is called again.
  void disconnect() {
    _closedByCaller = true;
    _isConnecting = false;
    _reconnectTimer?.cancel();
    _reconnectTimer = null;
    _retryDelay = initialRetryDelay;
    _teardownSocket();
  }
}
