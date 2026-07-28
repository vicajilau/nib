// ignore: unused_import
import 'package:intl/intl.dart' as intl;
import 'app_localizations.dart';

// ignore_for_file: type=lint

/// The translations for English (`en`).
class AppLocalizationsEn extends AppLocalizations {
  AppLocalizationsEn([String locale = 'en']) : super(locale);

  @override
  String get appTitle => 'Nib';

  @override
  String get waitingForHost => 'Waiting for Nib host...';

  @override
  String get nibActive => 'Nib Active';

  @override
  String get gesturesGuide => 'Gestures Guide';

  @override
  String get hideGuide => 'Hide Guide';

  @override
  String get trackpadControls => 'Trackpad & Gesture Controls';

  @override
  String get oneFingerTapTitle => '1 Finger Tap';

  @override
  String get oneFingerTapSubtitle => 'Left Click & Selection Drag';

  @override
  String get twoFingersTapTitle => '2 Fingers Tap';

  @override
  String get twoFingersTapSubtitle => 'Right Click (Context Menu)';

  @override
  String get twoFingersSwipeTitle => '2 Fingers Swipe';

  @override
  String get twoFingersSwipeSubtitle => 'Smooth 2-Axis Scroll';

  @override
  String get threeFingersSwipeTitle => '3 Fingers Swipe';

  @override
  String get threeFingersSwipeSubtitle => 'GNOME Workspaces & Overview';

  @override
  String get stylusTitle => 'Stylus / Apple Pencil';

  @override
  String get stylusSubtitle => 'Pressure & Inclinative Drawing';
}
