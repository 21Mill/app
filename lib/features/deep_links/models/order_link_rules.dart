import 'package:mostro/core/app_routes.dart';
import 'package:mostro/shared/utils/platform_int64.dart';
import 'package:mostro/src/rust/api/types.dart';

/// Pure rules of opening a `mostro:` order link — no widgets, no providers.
/// The link's format and the lookup of its order live in Rust
/// (`rust/src/nostr/order_link.rs`).

/// The scheme a node writes into an order's `source` tag.
const orderLinkScheme = 'mostro';

/// Whether [raw] is meant for this app: a `mostro:` link, in any case. Whether
/// it is a *valid* one is the core's to say.
bool isOrderLink(String raw) =>
    raw.trim().toLowerCase().startsWith('$orderLinkScheme:');

/// The order link [appUrl] carries, or `null` when it carries none.
///
/// This is how a link reaches the web: a page cannot be the handler of a
/// `mostro:` link, so whoever shares one shares the address of the web client
/// with the link in its query, and that address is the app's own. The web
/// build uses the hash strategy — there is no `usePathUrlStrategy` call — so
/// the route lives in the fragment: `https://host/app/#/order_link?link=...`.
///
/// Parsed from the raw text, after the `#`, and never from `Uri.fragment`,
/// which is already decoded: a link's own `&` would come back as a separator
/// and cut the link short at the first relay.
String? orderLinkFromAppUrl(String appUrl) {
  final hash = appUrl.indexOf('#');
  if (hash < 0) return null;
  final route = Uri.tryParse(appUrl.substring(hash + 1));
  if (route == null || route.path != AppRoute.orderLink) return null;
  final link = route.queryParameters['link']?.trim();
  return link != null && isOrderLink(link) ? link : null;
}

/// Why an order link did not lead to an order.
enum OrderLinkFailure {
  /// The link cannot be read: not an order link, or part of it is missing.
  invalidLink,

  /// No relay answered with the order.
  notFound,

  /// The order exists, but can no longer be taken and the user has no part
  /// in it.
  unavailable,

  /// Changing to the order's node failed.
  nodeChangeFailed,

  /// The trades could not be read, so whether a node change would leave one
  /// behind is unknown.
  tradesUnreadable,
}

/// The failure a `resolveOrderLink` error stands for. The core reports
/// `InvalidOrderLink` and `OrderLinkNotFound` markers; anything else (the
/// bridge, the relays) is as good as not found, and as worth retrying.
OrderLinkFailure orderLinkFailureOf(Object error) =>
    error.toString().contains('InvalidOrderLink')
        ? OrderLinkFailure.invalidLink
        : OrderLinkFailure.notFound;

/// Where an order link leads once its order was found on the active node, or
/// `null` when it leads nowhere ([OrderLinkFailure.unavailable]).
///
/// A participant goes to their trade, where the take screen would send them
/// anyway (#268), and a pending order to the take screen of its side: taking a
/// sell order is buying. A pending order whose take window closed by [now]
/// leads nowhere: the node refuses to take it, and one that no longer holds
/// the order never publishes its expiry, so the pending revision outlives it
/// on the relays.
String? orderLinkDestination(
  OrderInfo order, {
  required bool participates,
  required DateTime now,
}) {
  if (participates) return AppRoute.tradeDetailPath(order.id);
  if (order.status != OrderStatus.pending) return null;
  final expiresAt = order.expiresAt;
  if (expiresAt != null &&
      platformInt64ToInt(expiresAt) * 1000 <= now.millisecondsSinceEpoch) {
    return null;
  }
  return switch (order.kind) {
    OrderKind.sell => AppRoute.takeSellPath(order.id),
    OrderKind.buy => AppRoute.takeBuyPath(order.id),
  };
}
