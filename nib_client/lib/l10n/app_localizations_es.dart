// ignore: unused_import
import 'package:intl/intl.dart' as intl;
import 'app_localizations.dart';

// ignore_for_file: type=lint

/// The translations for Spanish Castilian (`es`).
class AppLocalizationsEs extends AppLocalizations {
  AppLocalizationsEs([String locale = 'es']) : super(locale);

  @override
  String get appTitle => 'Nib';

  @override
  String get waitingForHost => 'Esperando al equipo Nib...';

  @override
  String get nibActive => 'Nib Activo';

  @override
  String get gesturesGuide => 'Guía de Gestos';

  @override
  String get hideGuide => 'Ocultar Guía';

  @override
  String get trackpadControls => 'Gestos y Controles Táctiles';

  @override
  String get oneFingerTapTitle => '1 Dedo (Toque)';

  @override
  String get oneFingerTapSubtitle => 'Clic Izquierdo y Modo Selección';

  @override
  String get twoFingersTapTitle => '2 Dedos (Toque)';

  @override
  String get twoFingersTapSubtitle => 'Clic Derecho (Menú Contextual)';

  @override
  String get twoFingersSwipeTitle => '2 Dedos (Desplazamiento)';

  @override
  String get twoFingersSwipeSubtitle => 'Desplazamiento Suave Continuo';

  @override
  String get threeFingersSwipeTitle => '3 Dedos (Desplazamiento)';

  @override
  String get threeFingersSwipeSubtitle =>
      'Escritorios Virtuales y Vista General';

  @override
  String get stylusTitle => 'Stylus / Apple Pencil';

  @override
  String get stylusSubtitle => 'Trazado con Presión e Inclinación';
}
