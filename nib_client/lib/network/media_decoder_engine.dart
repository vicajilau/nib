import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// Bridges to the platform-native H.264 hardware video decoder via a `MethodChannel`,
/// exposing the decoded frames as a texture and notifying listeners of connection state.
class MediaDecoderEngine extends ChangeNotifier {
  static const MethodChannel _channel = MethodChannel(
    'dev.victorcarreras.nib/decoder',
  );

  int? _textureId;
  bool _isConnected = false;

  /// Flutter texture ID backing the decoded video surface, or `null` if not decoding.
  int? get textureId => _textureId;
  /// Whether the native decoder currently has an active video stream connection.
  bool get isConnected => _isConnected;

  /// Registers the handler for native-to-Dart callbacks (`onConnected`, `onDisconnected`).
  MediaDecoderEngine() {
    _channel.setMethodCallHandler(_handleMethodCall);
  }

  /// Starts native hardware video decoding for the specified network port and display dimensions.
  Future<void> startDecoding({
    int port = 6000,
    required int width,
    required int height,
  }) async {
    try {
      final int? id = await _channel.invokeMethod<int>('startDecoding', {
        'port': port,
        'width': width,
        'height': height,
      });
      _textureId = id;
      notifyListeners();
    } catch (e) {
      debugPrint('Error starting native decoder: $e');
    }
  }

  /// Stops native hardware decoding and releases the associated texture.
  Future<void> stopDecoding() async {
    try {
      await _channel.invokeMethod('stopDecoding');
    } catch (e) {
      debugPrint('Error stopping native decoder: $e');
    } finally {
      _textureId = null;
      _isConnected = false;
      notifyListeners();
    }
  }

  /// Handles callbacks invoked from the native side to report decoder connection state.
  Future<dynamic> _handleMethodCall(MethodCall call) async {
    switch (call.method) {
      case 'onConnected':
        _isConnected = true;
        notifyListeners();
        break;
      case 'onDisconnected':
        _isConnected = false;
        notifyListeners();
        break;
      default:
        debugPrint(
          'Unknown method from native decoder channel: ${call.method}',
        );
    }
  }
}
