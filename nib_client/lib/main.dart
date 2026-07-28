import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'l10n/app_localizations.dart';
import 'ui/decoder_screen.dart';

/// Entry point for the Nib Flutter client.
///
/// Enables immersive sticky mode (hiding system bars) so the streamed desktop uses the
/// full tablet screen, then launches the app.
void main() {
  WidgetsFlutterBinding.ensureInitialized();
  SystemChrome.setEnabledSystemUIMode(SystemUiMode.immersiveSticky);
  runApp(const NibApp());
}

/// Root widget configuring the app's theme, localization, and initial screen.
class NibApp extends StatelessWidget {
  const NibApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Nib',
      debugShowCheckedModeBanner: false,
      localizationsDelegates: const [
        AppLocalizations.delegate,
        GlobalMaterialLocalizations.delegate,
        GlobalWidgetsLocalizations.delegate,
        GlobalCupertinoLocalizations.delegate,
      ],
      supportedLocales: AppLocalizations.supportedLocales,
      theme: ThemeData.dark().copyWith(
        scaffoldBackgroundColor: const Color(0xFF1E1E1E),
        colorScheme: const ColorScheme.dark(
          primary: Color(0xFF3584E4),
          secondary: Color(0xFF2EC27E),
          surface: Color(0xFF2D2D2D),
        ),
      ),
      home: const DecoderScreen(),
    );
  }
}
