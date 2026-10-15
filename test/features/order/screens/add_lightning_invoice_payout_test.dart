import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';

import 'package:mostro/core/app_routes.dart';
import 'package:mostro/core/app_theme.dart';
import 'package:mostro/features/about/providers/mostro_node_provider.dart';
import 'package:mostro/features/order/models/invoice_rules.dart';
import 'package:mostro/features/order/providers/invoice_providers.dart';
import 'package:mostro/features/order/providers/trade_state_provider.dart';
import 'package:mostro/features/order/screens/add_lightning_invoice_screen.dart';
import 'package:mostro/features/settings/providers/nwc_provider.dart';
import 'package:mostro/features/trades/providers/trades_providers.dart';
import 'package:mostro/l10n/app_localizations.dart';
import 'package:mostro/src/rust/api/types.dart';

import '../../../support/fake_payout.dart';

const _invoice = fakePayoutInvoice;

/// The add-invoice screen opened again after a failed payout (#823): the
/// default address failed three times, the NWC wallet "Alby" is connected,
/// and every submission is captured with how its destination was chosen.
Future<void> _pump(
  WidgetTester tester, {
  required List<InvoiceSubmission> sent,
  required Future<String> Function(int sats) generateInvoice,
}) async {
  final router = GoRouter(
    initialLocation: AppRoute.addInvoicePath('order-1'),
    routes: [
      GoRoute(
        path: AppRoute.addInvoice,
        builder:
            (_, state) => AddLightningInvoiceScreen(
              orderId: state.pathParameters['orderId']!,
              generateInvoice: generateInvoice,
            ),
      ),
      GoRoute(
        path: AppRoute.tradeDetail,
        builder: (_, __) => const Scaffold(body: Text('trade')),
      ),
    ],
  );
  addTearDown(router.dispose);
  await tester.pumpWidget(
    ProviderScope(
      overrides: [
        nwcProvider.overrideWith(
          (ref) =>
              NwcNotifier()..setConnected(
                NwcWalletState(
                  walletPubkey: 'wallet',
                  relayUrls: const [],
                  walletName: 'Alby',
                ),
              ),
        ),
        tradeAmountProvider.overrideWith(
          (ref, orderId) => Stream.value(BigInt.from(1677)),
        ),
        tradeUpdatesProvider.overrideWith(
          (ref) => const Stream<TradeUpdate>.empty(),
        ),
        tradeTouchProvider.overrideWith(
          (ref) => const Stream<TradeTouch>.empty(),
        ),
        tradeInfoProvider.overrideWith((ref, orderId) async => null),
        mostroNodeProvider.overrideWith((ref) async => null),
        invoiceCheckerProvider.overrideWithValue(
          (request) async => const InvoiceCheckValid(1677),
        ),
        payoutDestinationsReaderProvider.overrideWithValue(
          (orderId) async => [
            fakePayoutDestination(outcome: PayoutOutcome.failed, attempts: 3),
          ],
        ),
        invoiceSenderProvider.overrideWithValue((submission) async {
          sent.add(submission);
        }),
      ],
      child: MaterialApp.router(
        theme: buildDarkTheme(),
        locale: const Locale('en'),
        localizationsDelegates: AppLocalizations.localizationsDelegates,
        supportedLocales: AppLocalizations.supportedLocales,
        routerConfig: router,
      ),
    ),
  );
}

/// Frames enough for the amount, the record, the node, the wallet and the
/// submit; the invoice countdown never lets the screen settle.
Future<void> _frames(WidgetTester tester) async {
  for (var i = 0; i < 8; i++) {
    await tester.pump(const Duration(milliseconds: 100));
  }
}

void main() {
  testWidgets('the wallet invoice after a failed payout is sent as the '
      "wallet's, and the buyer is told why", (tester) async {
    // Arrange
    final sent = <InvoiceSubmission>[];
    final wallet = Completer<String>();
    await _pump(tester, sent: sent, generateInvoice: (_) => wallet.future);
    await _frames(tester);

    // While the wallet works, the buyer reads what failed and what is next.
    expect(
      find.text(
        'The payment to alice@example.com failed after 3 attempts. '
        'Alby is making a new invoice for these sats.',
      ),
      findsOneWidget,
    );

    // Act: the wallet answers; nobody taps anything.
    wallet.complete(_invoice);
    await _frames(tester);

    // Assert: the record says whose invoice it was.
    expect(sent, hasLength(1));
    expect(sent.single.invoice, _invoice);
    expect(sent.single.source, PayoutSource.nwcWallet);
    expect(sent.single.walletName, 'Alby');
    expect(find.text('trade'), findsOneWidget);
  });

  testWidgets('when the wallet fails, the invoice the buyer types is sent '
      'as entered', (tester) async {
    // Arrange
    final sent = <InvoiceSubmission>[];
    await _pump(
      tester,
      sent: sent,
      generateInvoice: (_) async => throw Exception('wallet offline'),
    );
    // The wallet's failure lands on the form by itself.
    await _frames(tester);
    // The form says why it is asked, and that the buyer adds it.
    expect(
      find.text(
        'The payment to alice@example.com failed after 3 attempts. '
        'Add a new invoice to receive your sats.',
      ),
      findsOneWidget,
    );

    // Act
    await tester.enterText(find.byType(TextField), _invoice);
    await tester.pump(const Duration(milliseconds: 400));
    await tester.tap(find.text('Send invoice'));
    await _frames(tester);

    // Assert
    expect(sent.single.source, PayoutSource.entered);
    expect(sent.single.walletName, isNull);
  });
}
