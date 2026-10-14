import 'package:flutter/foundation.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:mostro/features/order/providers/invoice_providers.dart';
import 'package:mostro/features/order/providers/trade_state_provider.dart';
import 'package:mostro/features/trades/providers/trades_providers.dart';
import 'package:mostro/src/rust/api/escrow_deadline.dart'
    as escrow_deadline_api;
import 'package:mostro/src/rust/api/types.dart' show OrderStatus;

/// When the node is estimated to act on a trade (unix seconds), or null when
/// that cannot be told. Behind a seam like the invoice step start.
final escrowDeadlineLookupProvider = Provider<Future<int?> Function(String)>(
  (ref) =>
      (orderId) async =>
          (await escrow_deadline_api.tradeEscrowDeadline(
            orderId: orderId,
          ))?.toInt(),
);

/// When mostrod's escrow deadline guardian is estimated to act on
/// [orderId]'s trade (unix seconds): it cancels an `active` trade and opens
/// a dispute on a `fiat-sent` one. Null in any other status, for a trade
/// whose escrow lock was never recorded, and on nodes without the guardian
/// (Cashu, older daemons). An estimate: Rust counts the node's blocks as ten
/// minutes each.
final escrowDeadlineProvider = FutureProvider.autoDispose.family<int?, String>((
  ref,
  orderId,
) async {
  final lookup = ref.watch(escrowDeadlineLookupProvider);
  // Re-evaluated whenever the trade record changes: the lock being recorded
  // and every status move both touch it.
  await ref.watch(tradeInfoProvider(orderId).future);
  try {
    return await lookup(orderId);
  } catch (e) {
    debugPrint('[escrowDeadline] unavailable: $e');
    return null;
  }
});

/// The deadline the trade screen counts down to: the node's window for the
/// waiting steps, the escrow deadline while the fiat leg runs, nothing else.
final tradeDeadlineProvider = FutureProvider.autoDispose.family<int?, String>((
  ref,
  orderId,
) async {
  switch (ref.watch(tradeStatusProvider(orderId)).valueOrNull) {
    case OrderStatus.waitingBuyerInvoice || OrderStatus.waitingPayment:
      return ref.watch(invoiceDeadlineProvider(orderId).future);
    case OrderStatus.active || OrderStatus.fiatSent:
      return ref.watch(escrowDeadlineProvider(orderId).future);
    default:
      return null;
  }
});
