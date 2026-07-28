use gtk4::glib;

/// Detects whether any of the user's configured system locales is Spanish (`es*`).
pub fn is_spanish() -> bool {
    let languages = glib::language_names();
    for lang in languages {
        if lang.starts_with("es") {
            return true;
        }
    }
    false
}

/// Translates a UI string key into English or Spanish based on the current system locale.
///
/// This is a lightweight alternative to gettext used for in-code UI strings; returns an
/// empty string for unknown keys.
pub fn tr(key: &str) -> &'static str {
    let spanish = is_spanish();
    match key {
        "app_title" => "Nib Display",
        "devices_group_title" => {
            if spanish {
                "Dispositivos Conectados"
            } else {
                "Connected Devices"
            }
        }
        "no_devices_title" => {
            if spanish {
                "No hay dispositivos Android/iOS conectados"
            } else {
                "No Android/iOS devices connected"
            }
        }
        "no_devices_subtitle" => {
            if spanish {
                "Conecta mediante USB-C con Depuración USB habilitada o por Wi-Fi"
            } else {
                "Connect via USB-C with USB Debugging enabled or over Wi-Fi"
            }
        }
        "gestures_group_title" => {
            if spanish {
                "Gestos y Controles de la Pantalla Táctil"
            } else {
                "Trackpad &amp; Gesture Controls"
            }
        }

        "row1_title" => {
            if spanish {
                "1 Dedo: Clic Izquierdo y Modo Selección"
            } else {
                "1 Finger: Left Click &amp; Selection Mode"
            }
        }
        "row1_subtitle" => {
            if spanish {
                "Toca para clic simple • Doble toque para activar modo selección (toca otra vez para soltar)"
            } else {
                "Tap for single click • Double-tap to activate selection mode (tap again to release)"
            }
        }

        "row2_title" => {
            if spanish {
                "2 Dedos (Toque): Clic Derecho (Menú Contextual)"
            } else {
                "2 Fingers Tap: Right Click (Context Menu)"
            }
        }
        "row2_subtitle" => {
            if spanish {
                "Toca con 2 dedos simultáneamente para abrir menú contextual"
            } else {
                "Tap with 2 fingers simultaneously to open right-click context menu"
            }
        }

        "row3_title" => {
            if spanish {
                "2 Dedos (Desplazamiento): Desplazamiento Suave"
            } else {
                "2 Fingers Swipe: Smooth Scroll"
            }
        }
        "row3_subtitle" => {
            if spanish {
                "Desliza arriba, abajo, izquierda o derecha para scroll continuo"
            } else {
                "Swipe up, down, left, or right for smooth 2-axis scrolling"
            }
        }

        "row4_title" => {
            if spanish {
                "3 Dedos: Escritorios Virtuales y Vista General de GNOME"
            } else {
                "3 Fingers Swipe: Virtual Desktops &amp; Overview"
            }
        }
        "row4_subtitle" => {
            if spanish {
                "Desliza arriba o abajo para alternar Espacios de Trabajo y Vista General de GNOME"
            } else {
                "Swipe up or down to toggle GNOME Workspaces &amp; Activities Overview"
            }
        }

        "start_stream" => {
            if spanish {
                "Iniciar transmisión"
            } else {
                "Start Stream"
            }
        }
        "stop_stream" => {
            if spanish {
                "Detener transmisión"
            } else {
                "Stop Stream"
            }
        }
        "device_disconnected" => {
            if spanish {
                "Dispositivo desconectado"
            } else {
                "Device disconnected"
            }
        }
        "stream_cancelled" => {
            if spanish {
                "Transmisión cancelada por el usuario"
            } else {
                "Stream request cancelled by user"
            }
        }
        "stream_error" => {
            if spanish {
                "Error al iniciar transmisión"
            } else {
                "Failed to start stream"
            }
        }

        "tray_show" => {
            if spanish {
                "Mostrar ventana principal"
            } else {
                "Show Main Window"
            }
        }
        "tray_quit" => {
            if spanish {
                "Salir"
            } else {
                "Quit"
            }
        }
        "tray_no_devices" => {
            if spanish {
                "No hay dispositivos conectados"
            } else {
                "No devices connected"
            }
        }
        "tray_streaming_active" => {
            if spanish {
                "Transmitiendo a"
            } else {
                "Streaming active to"
            }
        }
        "tray_devices_connected" => {
            if spanish {
                "Dispositivos conectados:"
            } else {
                "Devices connected:"
            }
        }

        "settings_title" => {
            if spanish {
                "Configuración de Pantalla y Transmisión"
            } else {
                "Display and Stream Settings"
            }
        }
        "settings_desc" => {
            if spanish {
                "Configura cómo la tablet muestra el escritorio GNOME"
            } else {
                "Configure how your tablet displays your GNOME desktop"
            }
        }

        "mode_title" => {
            if spanish {
                "Modo de Pantalla"
            } else {
                "Display Mode"
            }
        }
        "mode_subtitle" => {
            if spanish {
                "Extender escritorio como monitor secundario o duplicar pantalla principal"
            } else {
                "Extend desktop as secondary monitor or mirror main screen"
            }
        }
        "mode_extend" => {
            if spanish {
                "Extender escritorio (Monitor secundario)"
            } else {
                "Extend Desktop (Secondary Monitor)"
            }
        }
        "mode_mirror" => {
            if spanish {
                "Duplicar pantalla principal"
            } else {
                "Mirror Main Display"
            }
        }

        "res_title" => {
            if spanish {
                "Resolución"
            } else {
                "Resolution"
            }
        }
        "res_subtitle" => {
            if spanish {
                "Resolución del monitor virtual"
            } else {
                "Virtual monitor resolution"
            }
        }
        "res_native" => {
            if spanish {
                "2560x1600 (Nativa Tablet)"
            } else {
                "2560x1600 (Native Tablet)"
            }
        }

        "fps_title" => {
            if spanish {
                "Tasa de Refresco (FPS)"
            } else {
                "Framerate"
            }
        }
        "fps_subtitle" => {
            if spanish {
                "Tasa de refresco objetivo de la transmisión"
            } else {
                "Target stream refresh rate"
            }
        }
        "fps_60" => {
            if spanish {
                "60 FPS (Fluido)"
            } else {
                "60 FPS (Smooth)"
            }
        }
        "fps_120" => {
            if spanish {
                "120 FPS (Ultra Fluido)"
            } else {
                "120 FPS (Ultra Smooth)"
            }
        }
        "fps_30" => {
            if spanish {
                "30 FPS (Ahorro de energía)"
            } else {
                "30 FPS (Power Saver)"
            }
        }

        "stylus_title" => {
            if spanish {
                "Habilitar Lápiz Táctil y Presión"
            } else {
                "Enable Stylus and Pressure Input"
            }
        }
        "stylus_subtitle" => {
            if spanish {
                "Mapear lápiz óptico directamente como digitalizador virtual de GNOME"
            } else {
                "Map tablet pen directly to GNOME virtual digitizer"
            }
        }

        _ => "",
    }
}
