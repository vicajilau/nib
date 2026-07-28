import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import '../network/input_socket_client.dart';

/// Intercepts pointer, touch, active stylus, and multi-finger gesture events and routes them to `InputSocketClient`.
class StylusInputHandler {
  /// Reference to the underlying socket client for transmitting binary packets.
  final InputSocketClient inputSocket;

  int _touchCount = 0;
  Offset _twoFingerStart = Offset.zero;
  Offset _threeFingerStart = Offset.zero;

  /// Creates a new `StylusInputHandler` with the specified [inputSocket].
  StylusInputHandler(this.inputSocket);

  /// Processes pointer contact down events for single finger touch, multi-finger gestures, or active stylus contact.
  void handlePointerDown(
    PointerDownEvent event,
    Size screenSize, {
    Rect? videoBounds,
  }) {
    if (screenSize.width <= 0 || screenSize.height <= 0) return;

    _touchCount++;
    final double normX;
    final double normY;
    if (videoBounds != null &&
        videoBounds.width > 0 &&
        videoBounds.height > 0) {
      normX = ((event.localPosition.dx - videoBounds.left) / videoBounds.width)
          .clamp(0.0, 1.0);
      normY = ((event.localPosition.dy - videoBounds.top) / videoBounds.height)
          .clamp(0.0, 1.0);
    } else {
      normX = (event.localPosition.dx / screenSize.width).clamp(0.0, 1.0);
      normY = (event.localPosition.dy / screenSize.height).clamp(0.0, 1.0);
    }

    if (event.kind == PointerDeviceKind.stylus ||
        event.kind == PointerDeviceKind.invertedStylus) {
      final double pressure = event.pressure.clamp(0.0, 1.0);
      final double tiltX = event.tilt;
      inputSocket.sendStylusEvent('DOWN', normX, normY, pressure, tiltX: tiltX);
    } else {
      if (_touchCount == 1) {
        inputSocket.sendTouchEvent('DOWN', normX, normY, event.pointer);
      } else if (_touchCount == 2) {
        _twoFingerStart = event.position;
      } else if (_touchCount == 3) {
        _threeFingerStart = event.position;
      }
    }
  }

  /// Processes pointer move events for single finger movement, 2-finger scrolling, 3-finger swipes, or active stylus strokes.
  void handlePointerMove(
    PointerMoveEvent event,
    Size screenSize, {
    Rect? videoBounds,
  }) {
    if (screenSize.width <= 0 || screenSize.height <= 0) return;

    final double normX;
    final double normY;
    if (videoBounds != null &&
        videoBounds.width > 0 &&
        videoBounds.height > 0) {
      normX = ((event.localPosition.dx - videoBounds.left) / videoBounds.width)
          .clamp(0.0, 1.0);
      normY = ((event.localPosition.dy - videoBounds.top) / videoBounds.height)
          .clamp(0.0, 1.0);
    } else {
      normX = (event.localPosition.dx / screenSize.width).clamp(0.0, 1.0);
      normY = (event.localPosition.dy / screenSize.height).clamp(0.0, 1.0);
    }

    if (event.kind == PointerDeviceKind.stylus ||
        event.kind == PointerDeviceKind.invertedStylus) {
      final double pressure = event.pressure.clamp(0.0, 1.0);
      final double tiltX = event.tilt;
      inputSocket.sendStylusEvent('MOVE', normX, normY, pressure, tiltX: tiltX);
    } else {
      if (_touchCount == 1) {
        inputSocket.sendTouchEvent('MOVE', normX, normY, event.pointer);
      } else if (_touchCount == 2) {
        final double dx = event.delta.dx;
        final double dy = event.delta.dy;
        inputSocket.sendScrollEvent(dx, dy);
      } else if (_touchCount == 3) {
        final double dx = event.delta.dx;
        final double dy = event.delta.dy;
        inputSocket.sendSwipeGesture(dx, dy);
        final double dyTotal = event.position.dy - _threeFingerStart.dy;
        if (dyTotal.abs() > 120) {
          inputSocket.sendOverviewGesture();
          _threeFingerStart = event.position;
        }
      }
    }
  }

  /// Processes pointer lift/release events for touch release, right click triggers, or stylus lifting.
  void handlePointerUp(
    PointerUpEvent event,
    Size screenSize, {
    Rect? videoBounds,
  }) {
    if (screenSize.width <= 0 || screenSize.height <= 0) return;

    final double normX;
    final double normY;
    if (videoBounds != null &&
        videoBounds.width > 0 &&
        videoBounds.height > 0) {
      normX = ((event.localPosition.dx - videoBounds.left) / videoBounds.width)
          .clamp(0.0, 1.0);
      normY = ((event.localPosition.dy - videoBounds.top) / videoBounds.height)
          .clamp(0.0, 1.0);
    } else {
      normX = (event.localPosition.dx / screenSize.width).clamp(0.0, 1.0);
      normY = (event.localPosition.dy / screenSize.height).clamp(0.0, 1.0);
    }

    if (event.kind == PointerDeviceKind.stylus ||
        event.kind == PointerDeviceKind.invertedStylus) {
      inputSocket.sendStylusEvent('UP', normX, normY, 0.0);
    } else {
      if (_touchCount == 2) {
        final double dist = (event.position - _twoFingerStart).distance;
        if (dist < 20) {
          inputSocket.sendRightClickEvent(normX, normY);
        }
      }
      if (_touchCount == 1) {
        inputSocket.sendTouchEvent('UP', normX, normY, event.pointer);
      }
    }

    _touchCount = (_touchCount - 1).clamp(0, 10);
  }
}
