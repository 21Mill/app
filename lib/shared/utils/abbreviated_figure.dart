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
  // Each step is a thousand times the last. The suffix is picked from the
  // figure as shown, so a value that rounds up to the next one reads `1M`,
  // never `1,000k`.
  const scales = [1.0, 1e3, 1e6, 1e9];
  final suffixes = ['', 'k', 'M', billions];
  var i = scales.lastIndexWhere((s) => n >= s);
  if (i < 0) i = 0;
  if (i < scales.length - 1 && _shown(n / scales[i], whole: i == 0) >= 1000) {
    i++;
  }
  if (i == 0) return NumberFormat.decimalPattern(locale).format(n.round());
  return '${fmt.format(n / scales[i])}${suffixes[i]}';
}

/// [x] rounded as the figure shows it: whole below a thousand, one decimal
/// with a suffix.
double _shown(double x, {required bool whole}) =>
    whole ? x.roundToDouble() : (x * 10).roundToDouble() / 10;
