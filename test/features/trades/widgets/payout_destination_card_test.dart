import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:mostro/core/app_theme.dart';
import 'package:mostro/core/automation/automation_ids.dart';
import 'package:mostro/features/order/providers/trade_state_provider.dart';
import 'package:mostro/features/trades/widgets/payout_destination_card.dart';
import 'package:mostro/l10n/app_localizations.dart';
import 'package:mostro/src/rust/api/types.dart';

import '../../../support/fake_payout.dart';

/// The trade detail's payout card over [route], as the bridge would read it.
Future<void> _pump(WidgetTester tester, List<PayoutDestination> route) async {
  await tester.pumpWidget(
    ProviderScope(
      overrides: [
        payoutDestinationsReaderProvider.overrideWithValue(
          (orderId) async => route,
        ),
        tradeTouchProvider.overrideWith(
          (ref) => const Stream<TradeTouch>.empty(),
        ),
      ],
      child: MaterialApp(
        theme: buildDarkTheme(),
        locale: const Locale('en'),
        localizationsDelegates: AppLocalizations.localizationsDelegates,
        supportedLocales: AppLocalizations.supportedLocales,
        home: const Scaffold(
          body: SingleChildScrollView(
            child: PayoutDestinationCard(orderId: 'order-1'),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

/// The `trade.payout` readout's machine value, or null when it is absent.
String? _readout(WidgetTester tester) {
  final finder = find.bySemanticsIdentifier(AutomationIds.tradePayout);
  if (finder.evaluate().isEmpty) return null;
  return tester.getSemantics(finder).label;
}

void main() {
  testWidgets('the default address paid on the first try', (tester) async {
    final semantics = tester.ensureSemantics();
    // Arrange + Act
    await _pump(tester, [fakePayoutDestination(outcome: PayoutOutcome.paid)]);

    // Assert
    expect(find.text('Paid to'), findsOneWidget);
    expect(find.text('alice@example.com'), findsOneWidget);
    expect(find.text('Your default address'), findsOneWidget);
    expect(find.text('1,677 sats'), findsOneWidget);
    expect(find.text('Paid'), findsOneWidget);
    // An address has no hash this device could know, and nothing came before.
    expect(find.text('Payment hash'), findsNothing);
    expect(find.text('Tried before'), findsNothing);
    expect(_readout(tester), 'paid');
    semantics.dispose();
  });

  testWidgets(
    'the address failed and the NWC wallet was paid: both are shown',
    (tester) async {
      final semantics = tester.ensureSemantics();
      String? copied;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'Clipboard.setData') {
            copied = (call.arguments as Map)['text'] as String?;
          }
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.platform,
          null,
        ),
      );

      // Arrange + Act
      await _pump(tester, fakeFailedThenPaidRoute());

      // Assert: where it went, who chose it, and what was tried first.
      expect(find.text('Paid to'), findsOneWidget);
      expect(find.text('Alby (NWC)'), findsOneWidget);
      expect(find.text('Your NWC wallet'), findsOneWidget);
      expect(find.text('Tried before'), findsOneWidget);
      expect(
        find.text('alice@example.com · Failed after 3 attempts'),
        findsOneWidget,
      );
      expect(_readout(tester), 'paid');

      // The hash row copies the whole hash.
      await tester.tap(find.text('Payment hash'));
      await tester.pump();
      expect(
        copied,
        '0001020304050607080900010203040506070809000102030405060708090102',
      );
      expect(find.text('Payment hash copied'), findsOneWidget);
      semantics.dispose();
    },
  );

  testWidgets('an invoice the buyer entered, waiting for the node', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    await _pump(tester, [
      fakePayoutDestination(
        destination: fakePayoutInvoice,
        source: PayoutSource.entered,
        paymentHash: 'aa' * 32,
      ),
    ]);

    expect(find.text('Sending to'), findsOneWidget);
    expect(find.textContaining('Invoice lnbc16770n1p…'), findsOneWidget);
    expect(find.text('You, for this trade'), findsOneWidget);
    expect(find.text('Waiting'), findsOneWidget);
    expect(find.text('Payment hash'), findsOneWidget);
    expect(_readout(tester), 'submitted');
    semantics.dispose();
  });

  testWidgets('a failure not replaced yet reads as not paid', (tester) async {
    final semantics = tester.ensureSemantics();
    await _pump(tester, [
      fakePayoutDestination(outcome: PayoutOutcome.failed, attempts: 3),
    ]);

    expect(find.text('Not paid to'), findsOneWidget);
    expect(find.text('Failed after 3 attempts'), findsOneWidget);
    expect(_readout(tester), 'failed');
    semantics.dispose();
  });

  testWidgets('draws nothing when this device sent no destination', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    await _pump(tester, const []);

    expect(find.text('Paid to'), findsNothing);
    expect(find.text('Sending to'), findsNothing);
    expect(_readout(tester), isNull);
    semantics.dispose();
  });
}
