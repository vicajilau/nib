import 'package:flutter/material.dart';
import '../input/stylus_input_handler.dart';
import '../l10n/app_localizations.dart';
import '../network/input_socket_client.dart';
import '../network/media_decoder_engine.dart';

/// Full-screen video decoder view rendering the host's streamed desktop and forwarding
/// touch/stylus/gesture input back to it.
class DecoderScreen extends StatefulWidget {
  const DecoderScreen({super.key});

  @override
  State<DecoderScreen> createState() => _DecoderScreenState();
}

class _DecoderScreenState extends State<DecoderScreen> {
  late final MediaDecoderEngine _decoderEngine;
  late final InputSocketClient _inputSocket;
  late final StylusInputHandler _stylusHandler;

  /// Whether the floating status HUD pill is currently visible.
  bool _showHud = true;

  /// Whether the trackpad gesture guide overlay is currently visible.
  bool _showGestureGuide = false;

  /// Guards against starting the native decoder more than once across rebuilds.
  bool _hasStartedDecoding = false;

  @override
  void initState() {
    super.initState();
    _decoderEngine = MediaDecoderEngine();
    _inputSocket = InputSocketClient();
    _stylusHandler = StylusInputHandler(_inputSocket);

    _decoderEngine.addListener(_onDecoderStateChanged);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (!_hasStartedDecoding) {
      _startDecodingWhenSizeIsKnown();
    }
  }

  /// Starts native decoding once `MediaQuery` reports a real (non-zero) size.
  ///
  /// On some devices `MediaQuery.of(context).size` is still `Size.zero` the first
  /// time `didChangeDependencies` runs (before the first layout pass completes).
  /// Starting the native decoder with a 0x0 target size makes `MediaCodec` fail on
  /// every single frame, which then looks like an endless connect/disconnect loop.
  void _startDecodingWhenSizeIsKnown() {
    final mediaQuery = MediaQuery.of(context);
    final double dpr = mediaQuery.devicePixelRatio;
    final int physicalW = (mediaQuery.size.width * dpr).round();
    final int physicalH = (mediaQuery.size.height * dpr).round();

    if (physicalW <= 0 || physicalH <= 0) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _startDecodingWhenSizeIsKnown();
      });
      return;
    }

    _hasStartedDecoding = true;
    _decoderEngine.startDecoding(width: physicalW, height: physicalH);
  }

  /// Reacts to native decoder connection changes: connects/disconnects the input socket
  /// and briefly reveals the HUD when a stream becomes active.
  void _onDecoderStateChanged() {
    if (mounted) {
      setState(() {});
    }
    if (_decoderEngine.isConnected) {
      _inputSocket.connect();
      setState(() => _showHud = true);
      Future.delayed(const Duration(seconds: 5), () {
        if (mounted) setState(() => _showHud = false);
      });
    } else {
      _inputSocket.disconnect();
    }
  }

  @override
  void dispose() {
    _decoderEngine.removeListener(_onDecoderStateChanged);
    _decoderEngine.stopDecoding();
    _inputSocket.disconnect();
    super.dispose();
  }

  /// Builds the video surface (letterboxed to preserve aspect ratio), the status HUD,
  /// and the gesture guide overlay.
  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;

    return Scaffold(
      backgroundColor: const Color(0xFF1E1E1E),
      body: Stack(
        children: [
          // Texture or Waiting Screen
          Positioned.fill(
            child: LayoutBuilder(
              builder: (context, constraints) {
                final double screenW = constraints.maxWidth;
                final double screenH = constraints.maxHeight;

                // Dynamic aspect ratio derived directly from active display viewport
                final double videoAspectRatio = (screenH > 0)
                    ? (screenW / screenH)
                    : 1.0;
                double videoWidth = screenW;
                double videoHeight = screenW / videoAspectRatio;

                if (videoHeight > screenH) {
                  videoHeight = screenH;
                  videoWidth = screenH * videoAspectRatio;
                }

                final double left = (screenW - videoWidth) / 2.0;
                final double top = (screenH - videoHeight) / 2.0;
                final Rect videoBounds = Rect.fromLTWH(
                  left,
                  top,
                  videoWidth,
                  videoHeight,
                );

                return Listener(
                  onPointerDown: (e) => _stylusHandler.handlePointerDown(
                    e,
                    Size(screenW, screenH),
                    videoBounds: videoBounds,
                  ),
                  onPointerMove: (e) => _stylusHandler.handlePointerMove(
                    e,
                    Size(screenW, screenH),
                    videoBounds: videoBounds,
                  ),
                  onPointerUp: (e) => _stylusHandler.handlePointerUp(
                    e,
                    Size(screenW, screenH),
                    videoBounds: videoBounds,
                  ),
                  child: Stack(
                    children: [
                      Positioned.fill(child: Container(color: Colors.black)),
                      Positioned(
                        left: left,
                        top: top,
                        width: videoWidth,
                        height: videoHeight,
                        child:
                            (_decoderEngine.textureId != null &&
                                _decoderEngine.isConnected)
                            ? Texture(textureId: _decoderEngine.textureId!)
                            : Center(
                                child: Column(
                                  mainAxisSize: MainAxisSize.min,
                                  children: [
                                    const CircularProgressIndicator(
                                      color: Color(0xFF3584E4),
                                    ),
                                    const SizedBox(height: 16),
                                    Text(
                                      l10n.waitingForHost,
                                      style: const TextStyle(
                                        color: Colors.white70,
                                        fontSize: 16,
                                      ),
                                    ),
                                  ],
                                ),
                              ),
                      ),
                    ],
                  ),
                );
              },
            ),
          ),

          // Libadwaita Active Floating HUD Pill
          if (_decoderEngine.isConnected && (_showHud || _showGestureGuide))
            Positioned(
              top: 24,
              right: 24,
              child: Container(
                padding: const EdgeInsets.symmetric(
                  horizontal: 16,
                  vertical: 8,
                ),
                decoration: BoxDecoration(
                  color: const Color(0xEE2D2D2D),
                  borderRadius: BorderRadius.circular(30),
                  boxShadow: const [
                    BoxShadow(
                      color: Colors.black45,
                      blurRadius: 10,
                      offset: Offset(0, 4),
                    ),
                  ],
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Container(
                      width: 10,
                      height: 10,
                      decoration: const BoxDecoration(
                        color: Color(0xFF2EC27E),
                        shape: BoxShape.circle,
                      ),
                    ),
                    const SizedBox(width: 8),
                    Text(
                      l10n.nibActive,
                      style: const TextStyle(
                        color: Colors.white,
                        fontSize: 13,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                    const SizedBox(width: 12),
                    GestureDetector(
                      onTap: () => setState(
                        () => _showGestureGuide = !_showGestureGuide,
                      ),
                      child: Container(
                        padding: const EdgeInsets.symmetric(
                          horizontal: 10,
                          vertical: 4,
                        ),
                        decoration: BoxDecoration(
                          color: const Color(0xFF3584E4),
                          borderRadius: BorderRadius.circular(12),
                        ),
                        child: Text(
                          _showGestureGuide
                              ? l10n.hideGuide
                              : l10n.gesturesGuide,
                          style: const TextStyle(
                            color: Colors.white,
                            fontSize: 12,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),

          // Gestures Guide Overlay Dialog
          if (_showGestureGuide)
            Center(
              child: Container(
                width: 360,
                padding: const EdgeInsets.all(24),
                decoration: BoxDecoration(
                  color: const Color(0xFF2D2D2D),
                  borderRadius: BorderRadius.circular(20),
                  border: Border.all(color: Colors.white12),
                  boxShadow: const [
                    BoxShadow(color: Colors.black54, blurRadius: 20),
                  ],
                ),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      mainAxisAlignment: MainAxisAlignment.spaceBetween,
                      children: [
                        Text(
                          l10n.trackpadControls,
                          style: const TextStyle(
                            color: Colors.white,
                            fontSize: 16,
                            fontWeight: FontWeight.bold,
                          ),
                        ),
                        IconButton(
                          icon: const Icon(Icons.close, color: Colors.white70),
                          onPressed: () =>
                              setState(() => _showGestureGuide = false),
                        ),
                      ],
                    ),
                    const Divider(color: Colors.white12),
                    const SizedBox(height: 8),
                    _buildGestureRow(
                      Icons.touch_app,
                      l10n.oneFingerTapTitle,
                      l10n.oneFingerTapSubtitle,
                    ),
                    _buildGestureRow(
                      Icons.mouse,
                      l10n.twoFingersTapTitle,
                      l10n.twoFingersTapSubtitle,
                    ),
                    _buildGestureRow(
                      Icons.swap_vert,
                      l10n.twoFingersSwipeTitle,
                      l10n.twoFingersSwipeSubtitle,
                    ),
                    _buildGestureRow(
                      Icons.grid_view,
                      l10n.threeFingersSwipeTitle,
                      l10n.threeFingersSwipeSubtitle,
                    ),
                    _buildGestureRow(
                      Icons.edit,
                      l10n.stylusTitle,
                      l10n.stylusSubtitle,
                    ),
                  ],
                ),
              ),
            ),
        ],
      ),
    );
  }

  /// Renders a single icon/title/subtitle row within the gesture guide overlay.
  Widget _buildGestureRow(IconData icon, String title, String subtitle) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8),
      child: Row(
        children: [
          Icon(icon, color: const Color(0xFF3584E4), size: 24),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title,
                  style: const TextStyle(
                    color: Colors.white,
                    fontSize: 13,
                    fontWeight: FontWeight.bold,
                  ),
                ),
                Text(
                  subtitle,
                  style: const TextStyle(color: Colors.white70, fontSize: 11),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
