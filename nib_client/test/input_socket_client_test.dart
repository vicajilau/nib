import 'dart:async';
import 'dart:io';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:nib_client/network/input_socket_client.dart';

/// Verifies that `InputSocketClient.buildBinaryPacket` encodes each event type into the
/// 20-byte little-endian binary layout expected by the host's `parse_binary_packet`.
void main() {
  group('InputSocketClient Binary Serialization Tests', () {
    test('buildBinaryPacket encodes TouchDown correctly', () {
      final pkt = InputSocketClient.buildBinaryPacket(
        eventType: 0x01, // TOUCH_DOWN
        slotOrFlags: 2,
        param1: 0.5,
        param2: 0.75,
      );

      expect(pkt.length, 20);
      expect(pkt[0], 0x53); // 'S'
      expect(pkt[1], 0x01);

      final bd = ByteData.sublistView(pkt);
      expect(bd.getUint16(2, Endian.little), 2);
      expect(bd.getFloat32(4, Endian.little), closeTo(0.5, 0.0001));
      expect(bd.getFloat32(8, Endian.little), closeTo(0.75, 0.0001));
    });

    test('buildBinaryPacket encodes StylusDown with pressure and tilt', () {
      final pkt = InputSocketClient.buildBinaryPacket(
        eventType: 0x04, // STYLUS_DOWN
        param1: 0.25,
        param2: 0.60,
        param3: 0.85,
        param4: 15.0,
      );

      expect(pkt.length, 20);
      expect(pkt[0], 0x53); // 'S'
      expect(pkt[1], 0x04);

      final bd = ByteData.sublistView(pkt);
      expect(bd.getFloat32(4, Endian.little), closeTo(0.25, 0.0001));
      expect(bd.getFloat32(8, Endian.little), closeTo(0.60, 0.0001));
      expect(bd.getFloat32(12, Endian.little), closeTo(0.85, 0.0001));
      expect(bd.getFloat32(16, Endian.little), closeTo(15.0, 0.0001));
    });

    test('buildBinaryPacket encodes SwipeGesture correctly', () {
      final pkt = InputSocketClient.buildBinaryPacket(
        eventType: 0x0B, // SWIPE_GESTURE
        param1: 12.5,
        param2: -45.0,
      );

      expect(pkt.length, 20);
      expect(pkt[0], 0x53);
      expect(pkt[1], 0x0B);

      final bd = ByteData.sublistView(pkt);
      expect(bd.getFloat32(4, Endian.little), closeTo(12.5, 0.0001));
      expect(bd.getFloat32(8, Endian.little), closeTo(-45.0, 0.0001));
    });
  });

  group('InputSocketClient connection lifecycle', () {
    late ServerSocket server;
    late InputSocketClient client;

    setUp(() async {
      server = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
      client = InputSocketClient();
    });

    tearDown(() async {
      client.disconnect();
      await server.close();
    });

    /// Polls [condition] until it holds or [timeout] elapses, so a test never hangs on a
    /// state change that fails to arrive.
    Future<void> waitUntil(
      bool Function() condition, {
      Duration timeout = const Duration(seconds: 5),
    }) async {
      final deadline = DateTime.now().add(timeout);
      while (!condition() && DateTime.now().isBefore(deadline)) {
        await Future<void>.delayed(const Duration(milliseconds: 10));
      }
    }

    test('connects and delivers a packet to the host', () async {
      final received = Completer<Uint8List>();
      server.listen((socket) {
        socket.listen(received.complete);
      });

      await client.connect(host: server.address.address, port: server.port);
      expect(client.isConnected, isTrue);

      client.sendRightClickEvent(0.5, 0.5);
      final bytes = await received.future.timeout(const Duration(seconds: 5));

      expect(bytes.length, 20);
      expect(bytes[0], 0x53);
      expect(bytes[1], 0x08);
    });

    test('notices when the host closes the connection', () async {
      Socket? accepted;
      server.listen((socket) => accepted = socket);

      await client.connect(host: server.address.address, port: server.port);
      expect(client.isConnected, isTrue);

      await waitUntil(() => accepted != null);
      await accepted!.close();

      // Before the socket had a subscription, no drop was ever observed here and isConnected
      // stayed true for the rest of the session.
      await waitUntil(() => !client.isConnected);
      expect(client.isConnected, isFalse);
    });

    test('reconnects on its own after the host drops it', () async {
      final connections = <Socket>[];
      server.listen(connections.add);

      await client.connect(host: server.address.address, port: server.port);
      await waitUntil(() => connections.isNotEmpty);
      await connections.first.close();

      await waitUntil(() => client.isConnected == false);
      await waitUntil(
        () => client.isConnected,
        timeout: const Duration(seconds: 10),
      );

      expect(client.isConnected, isTrue);
      expect(connections.length, greaterThanOrEqualTo(2));
    });

    test('does not reconnect after an explicit disconnect', () async {
      final connections = <Socket>[];
      server.listen(connections.add);

      await client.connect(host: server.address.address, port: server.port);
      await waitUntil(() => connections.isNotEmpty);

      client.disconnect();
      expect(client.isConnected, isFalse);

      // Sending after a deliberate teardown must stay a no-op rather than resurrecting the
      // connection behind the caller's back.
      client.sendRightClickEvent(0.5, 0.5);
      await Future<void>.delayed(const Duration(milliseconds: 600));

      expect(client.isConnected, isFalse);
      expect(connections.length, 1);
    });

    test('gives up gracefully when nothing is listening', () async {
      final closedPort = server.port;
      await server.close();

      await client.connect(
        host: InternetAddress.loopbackIPv4.address,
        port: closedPort,
      );

      expect(client.isConnected, isFalse);
      // Must not throw: a failed connect only queues a retry.
      client.sendRightClickEvent(0.5, 0.5);

      server = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    });
  });
}
