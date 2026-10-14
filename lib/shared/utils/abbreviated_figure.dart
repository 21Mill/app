import 'package:intl/intl.dart';

/// `5000` → `5k`, `2000000` → `2M`, `1500` → `1.5k` (locale separator),
/// `950` → `950`. Billions take [billions]: `B` for a fiat figure, `G` where
/// a caller already reads sats that way.
///
/// The suffixes are the same in every locale on purpose: the locale's compact
/// format spells them out (`61,2 mil` in Spanish), which a card has no room
/// for.
String abbreviateFigure(num value, String locale, {String billions = 'B'}) {
  final n = value.toDouble();
  final fmt =
      NumberFormat.decimalPattern(locale)
        ..minimumFractionDigits = 0
        ..maximumFractionDigits = 1;
  if (n >= 1e9) return '${fmt.format(n / 1e9)}$billions';
  if (n >= 1e6) return '${fmt.format(n / 1e6)}M';
  if (n >= 1e3) return '${fmt.format(n / 1e3)}k';
  return NumberFormat.decimalPattern(locale).format(n.round());
}
