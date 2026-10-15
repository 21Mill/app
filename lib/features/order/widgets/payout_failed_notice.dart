import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:mostro/core/automation/automation_id.dart';
import 'package:mostro/core/automation/automation_ids.dart';
import 'package:mostro/features/order/providers/trade_state_provider.dart';
import 'package:mostro/features/order/widgets/invoice_widgets.dart';
import 'package:mostro/features/settings/providers/nwc_provider.dart';
import 'package:mostro/features/trades/models/payout_route_rules.dart';
import 'package:mostro/l10n/app_localizations.dart';

/// Why the add-invoice screen is open again (#823): the node could not pay
/// the destination the buyer gave, and where the next invoice comes from —
/// the NWC wallet, on its own, or the buyer. Nothing when the screen asks
/// for a first invoice, or when this device sent no earlier destination.
class PayoutFailedNotice extends ConsumerWidget {
  const PayoutFailedNotice({
    super.key,
    required this.orderId,
    required this.walletMakesInvoice,
  });

  final String orderId;

  /// The connected NWC wallet generates the next invoice and the screen
  /// sends it without a tap.
  final bool walletMakesInvoice;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final route =
        ref.watch(payoutDestinationsProvider(orderId)).valueOrNull ?? const [];
    final failed = unreplacedFailure(route);
    if (failed == null) return const SizedBox.shrink();
    final l10n = AppLocalizations.of(context);
    final where = destinationLabel(l10n, failed);
    final attempts = failed.attempts;
    final what =
        attempts == null
            ? l10n.invoicePayoutFailed(where)
            : l10n.invoicePayoutFailedAttempts(where, attempts);
    final wallet = ref.watch(nwcProvider)?.walletName?.trim();
    final next =
        !walletMakesInvoice
            ? l10n.invoicePayoutFailedManual
            : wallet == null || wallet.isEmpty
            ? l10n.invoicePayoutFailedNwc
            : l10n.invoicePayoutFailedNwcNamed(wallet);
    final text = '$what $next';
    return InvoiceValidationRow(
      text: text,
      isValid: false,
    ).withAutomationId(AutomationIds.invoicePayoutFailed, label: text);
  }
}
