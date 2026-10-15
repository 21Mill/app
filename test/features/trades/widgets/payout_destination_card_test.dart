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
Future<void> _pump(
  WidgetTester tester,
  List<PayoutDestination> route, {
  OrderStatus status = OrderStatus.settledHoldInvoice,
}) async {
  await tester.pumpWidget(
    ProviderScope(
      overrides: [
        tradeStatusProvider.overrideWith(
          (ref, orderId) => Stream.value(status),
        ),
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

      // The wallet's name alone does not say which invoice: its row does,
      // and copies it whole (PR #824 review).
      expect(find.text('Invoice'), findsOneWidget);
      await tester.tap(find.text('Invoice'));
      await tester.pump();
      expect(copied, fakePayoutInvoice);
      semantics.dispose();
    },
  );

  testWidgets(
    'a destination tried before opens to its own details (PR #824 review)',
    (tester) async {
      // Arrange
      await _pump(tester, fakeFailedThenPaidRoute());
      expect(find.text('Your default address'), findsNothing);

      // Act
      await tester.tap(find.text('Tried before'));
      await tester.pumpAndSettle();

      // Assert: who chose it and what it was for; tapping again closes it.
      expect(find.text('Your default address'), findsOneWidget);
      expect(find.text('1,677 sats'), findsNWidgets(2));
      await tester.tap(find.text('Tried before'));
      await tester.pumpAndSettle();
      expect(find.text('Your default address'), findsNothing);
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

  testWidgets('before the release it only says where the sats will go '
      '(PR #824 review)', (tester) async {
    final semantics = tester.ensureSemantics();
    for (final status in [
      OrderStatus.waitingPayment,
      OrderStatus.active,
      OrderStatus.fiatSent,
    ]) {
      await _pump(tester, [fakePayoutDestination()], status: status);

      expect(find.text('Sats will go to'), findsOneWidget, reason: '$status');
      expect(find.text('alice@example.com'), findsOneWidget);
      expect(find.text('Sending to'), findsNothing, reason: '$status');
      expect(find.text('Waiting'), findsNothing, reason: '$status');
      expect(_readout(tester), 'scheduled');
    }
    semantics.dispose();
  });

  testWidgets('a failed payout stays shown while a new invoice is asked for', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    // The node asks again: the trade reads waiting-buyer-invoice, but the
    // payout has started and its failure is the news.
    await _pump(tester, [
      fakePayoutDestination(outcome: PayoutOutcome.failed, attempts: 3),
    ], status: OrderStatus.waitingBuyerInvoice);

    expect(find.text('Not paid to'), findsOneWidget);
    expect(find.text('Failed after 3 attempts'), findsOneWidget);
    expect(_readout(tester), 'failed');
    semantics.dispose();
  });

  testWidgets('a payment the node did not tie to one destination is not '
      'guessed (PR #824 review)', (tester) async {
    final semantics = tester.ensureSemantics();
    await _pump(tester, [
      fakePayoutDestination(outcome: PayoutOutcome.maybePaid),
      fakePayoutDestination(
        destination: 'bob@example.org',
        source: PayoutSource.entered,
        outcome: PayoutOutcome.maybePaid,
      ),
    ], status: OrderStatus.success);

    expect(find.text('Maybe paid to'), findsOneWidget);
    expect(find.text('bob@example.org'), findsOneWidget);
    expect(
      find.text('Paid to this one or another listed here'),
      findsOneWidget,
    );
    expect(
      find.text('alice@example.com · Paid to this one or another listed here'),
      findsOneWidget,
    );
    expect(find.text('Paid to'), findsNothing);
    expect(_readout(tester), 'maybe-paid');
    semantics.dispose();
  });

  testWidgets('a destination a later one replaced says so', (tester) async {
    await _pump(tester, [
      fakePayoutDestination(outcome: PayoutOutcome.replaced),
      fakePayoutDestination(
        destination: 'bob@example.org',
        source: PayoutSource.entered,
      ),
    ]);

    expect(
      find.text('alice@example.com · Replaced by a later one'),
      findsOneWidget,
    );
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
