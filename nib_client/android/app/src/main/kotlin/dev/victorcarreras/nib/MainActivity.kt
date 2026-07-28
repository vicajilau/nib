package dev.victorcarreras.nib

import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.Surface
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import io.flutter.view.TextureRegistry
import java.io.DataInputStream
import java.io.EOFException
import java.net.Socket
import java.net.SocketTimeoutException
import kotlin.concurrent.thread

/**
 * Native Android host for the Nib video pipeline: connects to the host daemon's
 * TCP video socket, feeds raw H.264 frames into a hardware [MediaCodec] decoder, and renders
 * the decoded output onto a Flutter texture surface exposed via a method channel.
 */
class MainActivity : FlutterActivity() {
    private val CHANNEL = "dev.victorcarreras.nib/decoder"

    private var textureEntry: TextureRegistry.SurfaceTextureEntry? = null
    private var surface: Surface? = null
    private var mediaCodec: MediaCodec? = null
    @Volatile private var isRunning = false
    private var socket: Socket? = null
    private val mainHandler = Handler(Looper.getMainLooper())
    private var channel: MethodChannel? = null

    /** Registers the `dev.victorcarreras.nib/decoder` method channel handling `startDecoding`/`stopDecoding` calls from Dart. */
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)

        channel = MethodChannel(flutterEngine.dartExecutor.binaryMessenger, CHANNEL)
        channel?.setMethodCallHandler { call, result ->
            when (call.method) {
                "startDecoding" -> {
                    val displayMetrics = resources.displayMetrics
                    val port = call.argument<Int>("port") ?: 6000
                    val width = call.argument<Int>("width") ?: displayMetrics.widthPixels
                    val height = call.argument<Int>("height") ?: displayMetrics.heightPixels
                    stopDecodingInternal()

                    val entry = flutterEngine.renderer.createSurfaceTexture()
                    textureEntry = entry
                    val surfaceTexture = entry.surfaceTexture()
                    surfaceTexture.setDefaultBufferSize(width, height)

                    val s = Surface(surfaceTexture)
                    surface = s

                    startDecoderLoop(s, port, width, height)
                    result.success(entry.id())
                }
                "stopDecoding" -> {
                    stopDecodingInternal()
                    result.success(null)
                }
                else -> result.notImplemented()
            }
        }
    }

    /**
     * Runs on a background thread for the lifetime of the decoding session: repeatedly connects
     * to the host's local video TCP port, reads length-prefixed H.264 frames, lazily creates the
     * [MediaCodec] decoder on first frame, and reconnects automatically on disconnect/timeout.
     */
    private fun startDecoderLoop(surface: Surface, port: Int, width: Int, height: Int) {
        isRunning = true

        thread(name = "FlutterNibDecoderThread") {
            var hasLoggedWaiting = false
            while (isRunning) {
                var currentSocket: Socket? = null
                while (isRunning && currentSocket == null) {
                    try {
                        val s = Socket("127.0.0.1", port)
                        s.tcpNoDelay = true
                        s.soTimeout = 2000 // 2-second timeout to detect cable unplugging immediately
                        currentSocket = s
                        socket = s
                    } catch (_: Exception) {
                        if (!hasLoggedWaiting) {
                            Log.d(TAG, "Waiting for Nib host on 127.0.0.1:$port...")
                            hasLoggedWaiting = true
                        }
                        Thread.sleep(1000)
                    }
                }

                if (!isRunning || currentSocket == null) break

                if (!surface.isValid) {
                    Log.w(TAG, "Surface is not valid, aborting MediaCodec configure")
                    break
                }

                var receivedFirstFrame = false
                try {
                    val dataInputStream = DataInputStream(currentSocket.getInputStream())

                    while (isRunning && currentSocket.isConnected && !currentSocket.isClosed) {
                        val frameLen = dataInputStream.readInt()
                        if (frameLen <= 0 || frameLen > 10 * 1024 * 1024) {
                            Log.e(TAG, "Invalid frame length: $frameLen")
                            break
                        }

                        val frameData = ByteArray(frameLen)
                        dataInputStream.readFully(frameData)

                        if (!receivedFirstFrame) {
                            receivedFirstFrame = true
                            hasLoggedWaiting = false
                            mainHandler.post { channel?.invokeMethod("onConnected", null) }
                            Log.i(TAG, "SUCCESS! First video frame received from Nib host (${width}x${height})")
                        }

                        if (mediaCodec == null) {
                            val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, width, height).apply {
                                setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, width * height * 3)
                                setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
                                setInteger(MediaFormat.KEY_PRIORITY, 0)
                            }
                            mediaCodec = MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_VIDEO_AVC).apply {
                                configure(format, surface, null, 0)
                                start()
                            }
                        }

                        feedCodec(frameData, frameLen)
                    }
                } catch (_: SocketTimeoutException) {
                    Log.d(TAG, "Socket read timeout (cable unplugged or host stopped)")
                } catch (_: EOFException) {
                    if (receivedFirstFrame) {
                        Log.i(TAG, "Host video stream closed (EOF)")
                    } else if (!hasLoggedWaiting) {
                        Log.d(TAG, "Waiting for Nib host video stream...")
                        hasLoggedWaiting = true
                    }
                } catch (e: Exception) {
                    Log.e(TAG, "Error in stream read: ${e.message ?: e.javaClass.simpleName}")
                }

                try { currentSocket.close() } catch (_: Exception) {}
                socket = null
                if (mediaCodec != null) {
                    try {
                        mediaCodec?.stop()
                        mediaCodec?.release()
                    } catch (_: Exception) {}
                    mediaCodec = null
                }
                mainHandler.post { channel?.invokeMethod("onDisconnected", null) }
                Thread.sleep(1000)
            }
        }
    }

    /** Queues a single received H.264 frame into the decoder's input buffer and drains any ready output buffers to the surface. */
    private fun feedCodec(data: ByteArray, length: Int) {
        val codec = mediaCodec ?: return
        try {
            val inputIndex = codec.dequeueInputBuffer(10_000L)
            if (inputIndex >= 0) {
                val inputBuffer = codec.getInputBuffer(inputIndex) ?: return
                inputBuffer.clear()
                inputBuffer.put(data, 0, length)
                codec.queueInputBuffer(inputIndex, 0, length, System.nanoTime() / 1000, 0)
            }

            val bufferInfo = MediaCodec.BufferInfo()
            var outputIndex = codec.dequeueOutputBuffer(bufferInfo, 0L)
            while (outputIndex >= 0) {
                codec.releaseOutputBuffer(outputIndex, true)
                outputIndex = codec.dequeueOutputBuffer(bufferInfo, 0L)
            }
        } catch (e: Exception) {
            Log.w(TAG, "Buffer processing exception: ${e.message}")
        }
    }

    /** Stops the decoder loop and releases the socket, [MediaCodec], surface, and texture entry. */
    private fun stopDecodingInternal() {
        isRunning = false
        try {
            socket?.close()
            mediaCodec?.stop()
            mediaCodec?.release()
        } catch (_: Exception) {}
        socket = null
        mediaCodec = null
        surface?.release()
        surface = null
        textureEntry?.release()
        textureEntry = null
    }

    companion object {
        private const val TAG = "FlutterNibDecoder"
    }
}
