import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:mostro/core/app_theme.dart';
import 'package:mostro/core/automation/automation_id.dart';
import 'package:mostro/core/automation/automation_ids.dart';
import 'package:mostro/features/order/models/invoice_rules.dart';
import 'package:mostro/features/order/providers/trade_state_provider.dart';
import 'package:mostro/features/order/widgets/order_detail_cards.dart';
import 'package:mostro/features/trades/models/payout_route_rules.dart';
import 'package:mostro/l10n/app_localizations.dart';
import 'package:mostro/src/rust/api/types.dart'
    show PayoutDestination, PayoutOutcome, PayoutSource;

/// Where the buyer's sats went (#823): the destination the node paid, or the
/// one it is paying, how it was chosen, the amount, what became of it, the
/// payment hash to find it in a wallet, and every destination tried before.
///
/// Built from what this device sent (`payout_destinations`), so it says
/// nothing for a trade whose destinations were sent from another device,
/// and draws nothing then. Shown to the buyer only.
class PayoutDestinationCard extends ConsumerWidget {
  const PayoutDestinationCard({super.key, required this.orderId});

  final String orderId;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final route =
        ref.watch(payoutDestinationsProvider(orderId)).valueOrNull ?? const [];
    final lead = leadingDestination(route);
    if (lead == null) return const SizedBox.shrink();
    final started = payoutHasStarted(
      ref.watch(tradeStatusProvider(orderId)).valueOrNull,
      route,
    );
    return Padding(
      padding: const EdgeInsets.only(top: 12),
      child: PayoutDestinationRows(route: route, started: started),
    ).withAutomationId(
      AutomationIds.tradePayout,
      merge: false,
      label: started ? outcomeMachineName(lead.outcome) : 'scheduled',
    );
  }
}

/// The rows of [PayoutDestinationCard] over [route], which holds at least
/// one destination. Each destination tried before is one row that opens in
/// place to the same details as the leading one (PR #824 review).
class PayoutDestinationRows extends StatefulWidget {
  const PayoutDestinationRows({
    super.key,
    required this.route,
    this.started = true,
  });

  final List<PayoutDestination> route;

  /// The node is paying, or tried to ([payoutHasStarted]). Before that the
  /// card only says where the sats will go: no "sending", no status.
  final bool started;

  @override
  State<PayoutDestinationRows> createState() => _PayoutDestinationRowsState();
}

class _PayoutDestinationRowsState extends State<PayoutDestinationRows> {
  /// The earlier destinations opened, by [_key].
  final Set<String> _open = {};

  static String _key(PayoutDestination d) =>
      '${d.submittedAt}|${d.destination}';

  void _toggle(PayoutDestination d) => setState(() {
    final key = _key(d);
    if (!_open.remove(key)) _open.add(key);
  });

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context);
    final lead = leadingDestination(widget.route)!;
    return OrderDataCard(
      rows: [
        OrderDataRow(
          icon: Icons.bolt_rounded,
          label: switch (lead.outcome) {
            _ when !widget.started => l10n.payoutWillGoToLabel,
            PayoutOutcome.paid => l10n.payoutPaidToLabel,
            PayoutOutcome.maybePaid => l10n.payoutMaybePaidToLabel,
            PayoutOutcome.submitted ||
            PayoutOutcome.retrying => l10n.payoutSendingToLabel,
            PayoutOutcome.failed ||
            PayoutOutcome.rejected ||
            PayoutOutcome.replaced => l10n.payoutNotPaidLabel,
          },
          value: OrderDataValue(destinationLabel(l10n, lead)),
        ),
        if (widget.started)
          OrderDataRow(
            icon: Icons.info_outline_rounded,
            label: l10n.statusLabel,
            value: OrderDataValue(outcomeLabel(l10n, lead)),
          ),
        ..._details(context, l10n, lead),
        for (final earlier in earlierDestinations(widget.route)) ...[
          _earlierRow(l10n, earlier),
          if (_open.contains(_key(earlier)))
            ..._details(context, l10n, earlier),
        ],
      ],
    );
  }

  /// `Tried before · alice@example.com · Failed ⌄`: opens or closes the
  /// destination's details in place.
  Widget _earlierRow(AppLocalizations l10n, PayoutDestination d) {
    final open = _open.contains(_key(d));
    final book = OrderBookPalette.of(context);
    return Semantics(
      expanded: open,
      child: OrderDataRow(
        icon: Icons.history_rounded,
        label: l10n.payoutTriedBeforeLabel,
        value: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Flexible(
              child: OrderDataValue(
                l10n.payoutTriedBeforeValue(
                  destinationLabel(l10n, d),
                  outcomeLabel(l10n, d),
                ),
              ),
            ),
            const SizedBox(width: 4),
            Icon(
              open ? Icons.expand_less : Icons.expand_more,
              size: 16,
              color: book.textTertiary,
            ),
          ],
        ),
        onTap: () => _toggle(d),
      ),
    );
  }

  /// Who chose [d], what it pays, and what identifies it in a wallet: the
  /// invoice an NWC wallet made (its name alone does not say which one) and
  /// the payment hash, each copied whole by its row.
  List<Widget> _details(
    BuildContext context,
    AppLocalizations l10n,
    PayoutDestination d,
  ) {
    final sats = d.amountSats?.toInt();
    final hash = d.paymentHash;
    final walletInvoice =
        !d.isLightningAddress && d.source == PayoutSource.nwcWallet;
    return [
      OrderDataRow(
        icon: Icons.person_outline_rounded,
        label: l10n.payoutChosenByLabel,
        value: OrderDataValue(sourceLabel(l10n, d.source)),
      ),
      if (sats != null)
        OrderDataRow(
          icon: Icons.account_balance_wallet_outlined,
          label: l10n.payoutAmountLabel,
          value: OrderDataValue(
            l10n.satsAmount(formatInvoiceSats(sats, l10n.localeName)),
            figures: true,
          ),
        ),
      if (walletInvoice)
        OrderDataRow(
          icon: Icons.receipt_long_outlined,
          label: l10n.payoutInvoiceLabel,
          value: _CopyValue(text: shortInvoice(d.destination)),
          onTap: () => _copy(context, d.destination, l10n.payoutInvoiceCopied),
        ),
      if (hash != null)
        OrderDataRow(
          icon: Icons.tag_rounded,
          label: l10n.payoutHashLabel,
          value: _CopyValue(text: shortPaymentHash(hash)),
          onTap: () => _copy(context, hash, l10n.payoutHashCopied),
        ),
    ];
  }

  static void _copy(BuildContext context, String text, String confirmation) {
    Clipboard.setData(ClipboardData(text: text));
    HapticFeedback.selectionClick();
    showOrderDetailSnackBar(context, confirmation);
  }
}

/// A shortened value that the row copies whole: figures and the copy icon,
/// as the id row draws them (DS-CMP-22).
class _CopyValue extends StatelessWidget {
  const _CopyValue({required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    final book = OrderBookPalette.of(context);
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Flexible(
          child: Text(
            text,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              fontFamily: AppFonts.figures,
              fontSize: 12,
              fontWeight: FontWeight.w500,
              color: book.textStrong,
            ),
          ),
        ),
        const SizedBox(width: 8),
        Icon(Icons.copy_rounded, size: 16, color: book.limeIcon),
      ],
    );
  }
}
