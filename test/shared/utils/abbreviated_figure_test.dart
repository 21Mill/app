import 'package:flutter_test/flutter_test.dart';
import 'package:mostro/shared/utils/abbreviated_figure.dart';

void main() {
  group('abbreviateFigure', () {
    test('keeps one decimal below a magnitude boundary', () {
      expect(abbreviateFigure(999949, 'en'), '999.9k');
      expect(abbreviateFigure(999949, 'es'), '999,9k');
    });

    test('takes the next suffix when rounding reaches it', () {
      expect(abbreviateFigure(999.6, 'en'), '1k');
      expect(abbreviateFigure(999950, 'en'), '1M');
      expect(abbreviateFigure(999950, 'es'), '1M');
      expect(abbreviateFigure(999950000, 'en'), '1B');
      expect(abbreviateFigure(999950000, 'en', billions: 'G'), '1G');
    });
  });
}
