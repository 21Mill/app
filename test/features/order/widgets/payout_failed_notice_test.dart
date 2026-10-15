import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:mostro/core/app_theme.dart';
import 'package:mostro/core/automation/automation_ids.dart';
import 'package:mostro/features/order/providers/trade_state_provider.dart';
import 'package:mostro/features/order/widgets/payout_failed_notice.dart';
import 'package:mostro/features/settings/providers/nwc_provider.dart';
import 'package:mostro/l10n/app_localizations.dart';
import 'package:mostro/src/rust/api/types.dart';

import '../../../support/fake_payout.dart';

Future<void> _pump(
  WidgetTester tester, {
  required List<PayoutDestination> route,
  required bool walletMakesInvoice,
  String? walletName,
}) async {
  await tester.pumpWidget(
    ProviderScope(
      overrides: [
        payoutDestinationsReaderProvider.overrideWithValue(
          (orderId) async => route,
        ),
        tradeTouchProvider.overrideWith(
          (ref) => const Stream<TradeTouch>.empty(),
        ),
        nwcProvider.overrideWith((ref) {
          final notifier = NwcNotifier();
          if (walletMakesInvoice) {
            notifier.setConnected(
              NwcWalletState(
                walletPubkey: 'wallet',
                relayUrls: const [],
                walletName: walletName,
              ),
            );
          }
          return notifier;
        }),
      ],
      child: MaterialApp(
        theme: buildDarkTheme(),
        locale: const Locale('en'),
        localizationsDelegates: AppLocalizations.localizationsDelegates,
        supportedLocales: AppLocalizations.supportedLocales,
        home: Scaffold(
          body: PayoutFailedNotice(
            orderId: 'order-1',
            walletMakesInvoice: walletMakesInvoice,
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

final _failed = [
  fakePayoutDestination(outcome: PayoutOutcome.failed, attempts: 3),
];

void main() {
  testWidgets('names the failed address and the wallet making the next one', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    await _pump(
      tester,
      route: _failed,
      walletMakesInvoice: true,
      walletName: 'Alby',
    );

    expect(
      find.text(
        'The payment to alice@example.com failed after 3 attempts. '
        'Alby is making a new invoice for these sats.',
      ),
      findsOneWidget,
    );
    expect(
      find.bySemanticsIdentifier(AutomationIds.invoicePayoutFailed),
      findsOneWidget,
    );
    semantics.dispose();
  });

  testWidgets('a wallet without a name is still named as the NWC wallet', (
    tester,
  ) async {
    await _pump(tester, route: _failed, walletMakesInvoice: true);

    expect(
      find.textContaining('Your NWC wallet is making a new invoice'),
      findsOneWidget,
    );
  });

  testWidgets('without the wallet the buyer is asked for the invoice', (
    tester,
  ) async {
    await _pump(
      tester,
      route: [fakePayoutDestination(outcome: PayoutOutcome.failed)],
      walletMakesInvoice: false,
    );

    expect(
      find.text(
        'The payment to alice@example.com failed. '
        'Add a new invoice to receive your sats.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('nothing on a first request, or once a new invoice went out', (
    tester,
  ) async {
    await _pump(tester, route: const [], walletMakesInvoice: true);
    expect(find.byType(Text), findsNothing);

    await _pump(
      tester,
      route: [..._failed, fakePayoutDestination(destination: 'b@example.org')],
      walletMakesInvoice: false,
    );
    expect(find.byType(Text), findsNothing);
  });
}
