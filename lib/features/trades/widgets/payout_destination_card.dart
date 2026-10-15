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
    show PayoutDestination, PayoutOutcome;

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
    return Padding(
      padding: const EdgeInsets.only(top: 12),
      child: PayoutDestinationRows(route: route),
    ).withAutomationId(
      AutomationIds.tradePayout,
      merge: false,
      label: outcomeMachineName(lead.outcome),
    );
  }
}

/// The rows of [PayoutDestinationCard] over [route], which holds at least
/// one destination.
class PayoutDestinationRows extends StatelessWidget {
  const PayoutDestinationRows({super.key, required this.route});

  final List<PayoutDestination> route;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context);
    final lead = leadingDestination(route)!;
    final sats = lead.amountSats?.toInt();
    final hash = lead.paymentHash;
    return OrderDataCard(
      rows: [
        OrderDataRow(
          icon: Icons.bolt_rounded,
          label: switch (lead.outcome) {
            PayoutOutcome.paid => l10n.payoutPaidToLabel,
            PayoutOutcome.submitted ||
            PayoutOutcome.retrying => l10n.payoutSendingToLabel,
            PayoutOutcome.failed ||
            PayoutOutcome.rejected => l10n.payoutNotPaidLabel,
          },
          value: OrderDataValue(destinationLabel(l10n, lead)),
        ),
        OrderDataRow(
          icon: Icons.person_outline_rounded,
          label: l10n.payoutChosenByLabel,
          value: OrderDataValue(sourceLabel(l10n, lead.source)),
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
        OrderDataRow(
          icon: Icons.info_outline_rounded,
          label: l10n.statusLabel,
          value: OrderDataValue(outcomeLabel(l10n, lead)),
        ),
        if (hash != null)
          OrderDataRow(
            icon: Icons.tag_rounded,
            label: l10n.payoutHashLabel,
            value: _CopyValue(text: shortPaymentHash(hash)),
            onTap: () => _copyHash(context, hash),
          ),
        for (final earlier in earlierDestinations(route))
          OrderDataRow(
            icon: Icons.history_rounded,
            label: l10n.payoutTriedBeforeLabel,
            value: OrderDataValue(
              l10n.payoutTriedBeforeValue(
                destinationLabel(l10n, earlier),
                outcomeLabel(l10n, earlier),
              ),
            ),
          ),
      ],
    );
  }

  static void _copyHash(BuildContext context, String hash) {
    Clipboard.setData(ClipboardData(text: hash));
    HapticFeedback.selectionClick();
    showOrderDetailSnackBar(
      context,
      AppLocalizations.of(context).payoutHashCopied,
    );
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
