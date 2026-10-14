import 'package:flutter_test/flutter_test.dart';
import 'package:mostro/features/cashu/cashu_error_messages.dart';
import 'package:mostro/l10n/app_localizations_en.dart';

void main() {
  final l10n = AppLocalizationsEn();

  String messageFor(String tail) =>
      cashuErrorMessage(Exception('CashuUnsupportedUnit: $tail'), l10n);

  // Issue #391: a dollar or millisat token was refused as "carries no proof of
  // the mint's signature", which tells the user the token is forged. Rust
  // refuses it from the token alone, so the token is untouched — say so, and
  // say which unit it is in.
  test('a token in another unit names that unit', () {
    for (final (code, name) in [
      ('msat', 'millisats'),
      ('usd', 'dollars (USD)'),
      ('eur', 'euros (EUR)'),
      // A currency code the app has no name for is shown as the code.
      ('gbp', 'GBP'),
    ]) {
      expect(
        messageFor(code),
        'This wallet only holds sats, and this token is in $name. It was '
        'left untouched: you can redeem it in a wallet that supports that '
        'unit.',
      );
    }
  });

  // #810: an escrow for a trade on another node is refused before any swap.
  test('a trade on another node says so', () {
    expect(
      cashuErrorMessage(Exception('TradeOnOtherNode'), l10n),
      l10n.tradeOnOtherNode,
    );
  });

  test('a unit the app cannot name gets the generic message', () {
    const generic =
        'This wallet only holds sats, and this token is in another unit '
        '(such as millisats or dollars). It was left untouched: you can '
        'redeem it in a wallet that supports its unit.';

    expect(messageFor('other'), generic);
    // Rust never sends free text here, but if it did, none of it is shown:
    // the token's maker chose it.
    final hostile = messageFor('some custom unit');
    expect(hostile, generic);
    expect(hostile, isNot(contains('some')));
  });
}
