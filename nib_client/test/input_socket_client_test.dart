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
}
