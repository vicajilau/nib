import 'dart:async';
import 'dart:io';
import 'package:flutter/foundation.dart';

/// Low-latency TCP socket client for transmitting binary input events from mobile device to host daemon.
class InputSocketClient {
  Socket? _socket;
  bool _isConnecting = false;
  bool _isConnected = false;

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

  /// Establishes an asynchronous TCP connection to the host input server.
  Future<void> connect({String host = '127.0.0.1', int port = 6001}) async {
    if (_isConnected || _isConnecting) return;
    _isConnecting = true;

    try {
      debugPrint('Connecting Input Socket to $host:$port...');
      _socket = await Socket.connect(
        host,
        port,
        timeout: const Duration(seconds: 5),
      );
      _socket?.setOption(SocketOption.tcpNoDelay, true);
      _isConnected = true;
      debugPrint('SUCCESS: Input Socket Connected to $host:$port');
    } catch (e) {
      debugPrint('Input Socket connection failed ($host:$port): $e');
      _isConnected = false;
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
    if (!_isConnected) {
      connect();
      return;
    }
    try {
      _socket?.add(packet);
    } catch (e) {
      debugPrint('Error sending over input socket: $e');
      _isConnected = false;
      connect();
    }
  }

  /// Closes the input socket connection and cleans up resources.
  void disconnect() {
    _isConnecting = false;
    _isConnected = false;
    try {
      _socket?.destroy();
    } catch (e) {
      debugPrint('Error closing socket: $e');
    } finally {
      _socket = null;
    }
  }
}
