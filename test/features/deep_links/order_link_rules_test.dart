import 'package:flutter_test/flutter_test.dart';
import 'package:mostro/core/app_routes.dart';
import 'package:mostro/features/deep_links/models/order_link_rules.dart';
import 'package:mostro/src/rust/api/types.dart';

import '../../support/fake_trades.dart';

const _id = 'e215c07e-b1f9-45b0-9640-0295067ee99a';

OrderInfo _order({
  OrderKind kind = OrderKind.sell,
  OrderStatus status = OrderStatus.pending,
  int? expiresAt,
}) =>
    fakeTrade(
      orderId: _id,
      kind: kind,
      status: status,
      expiresAt: expiresAt,
    ).order;

/// When the link is opened, in unix seconds.
const _openedAt = 1778848774;
final _now = DateTime.fromMillisecondsSinceEpoch(_openedAt * 1000);

void main() {
  group('isOrderLink', () {
    test('takes a mostro: link in any case, with stray whitespace', () {
      expect(isOrderLink('mostro:$_id?relays=wss://nos.lol'), isTrue);
      expect(isOrderLink('  MOSTRO://$_id '), isTrue);
    });

    test('leaves every other link alone', () {
      expect(isOrderLink('https://mostro.network/app/'), isFalse);
      expect(isOrderLink('nostr:npub1abc'), isFalse);
      expect(isOrderLink('lightning:lnbc1'), isFalse);
      expect(isOrderLink(''), isFalse);
    });
  });

  group('orderLinkFailureOf', () {
    test('reads the core marker for a link it cannot read', () {
      expect(
        orderLinkFailureOf(Exception('InvalidOrderLink: order id is not')),
        OrderLinkFailure.invalidLink,
      );
    });

    test('takes everything else for an order not found, worth a retry', () {
      expect(
        orderLinkFailureOf(Exception('OrderLinkNotFound: $_id')),
        OrderLinkFailure.notFound,
      );
      expect(
        orderLinkFailureOf(StateError('bridge')),
        OrderLinkFailure.notFound,
      );
    });
  });

  group('orderLinkDestination', () {
    test('a pending sell order is taken by buying', () {
      expect(
        orderLinkDestination(_order(), participates: false, now: _now),
        AppRoute.takeSellPath(_id),
      );
    });

    test('a pending buy order is taken by selling', () {
      expect(
        orderLinkDestination(
          _order(kind: OrderKind.buy),
          participates: false,
          now: _now,
        ),
        AppRoute.takeBuyPath(_id),
      );
    });

    test('a participant goes to the trade, whatever the order says', () {
      for (final status in [OrderStatus.pending, OrderStatus.active]) {
        expect(
          orderLinkDestination(
            _order(status: status),
            participates: true,
            now: _now,
          ),
          AppRoute.tradeDetailPath(_id),
          reason: '$status',
        );
      }
    });

    // A pending revision can outlive its take window on the relays: a node
    // that no longer holds the order never publishes its expiry, and refuses
    // to take it. The take screen would only show a dead button.
    test('a pending order past its take window leads nowhere', () {
      for (final expiresAt in [_openedAt, _openedAt - 1]) {
        expect(
          orderLinkDestination(
            _order(expiresAt: expiresAt),
            participates: false,
            now: _now,
          ),
          isNull,
          reason: 'expires at $expiresAt',
        );
      }
    });

    test('a pending order inside its take window is taken', () {
      expect(
        orderLinkDestination(
          _order(expiresAt: _openedAt + 1),
          participates: false,
          now: _now,
        ),
        AppRoute.takeSellPath(_id),
      );
    });

    test('a participant goes to the trade past the take window too', () {
      expect(
        orderLinkDestination(
          _order(expiresAt: _openedAt - 1),
          participates: true,
          now: _now,
        ),
        AppRoute.tradeDetailPath(_id),
      );
    });

    test('an order that left the book leads nowhere for anyone else', () {
      for (final status in [
        OrderStatus.inProgress,
        OrderStatus.canceled,
        OrderStatus.success,
        OrderStatus.expired,
      ]) {
        expect(
          orderLinkDestination(
            _order(status: status),
            participates: false,
            now: _now,
          ),
          isNull,
          reason: '$status',
        );
      }
    });
  });

  group('orderLinkFromAppUrl', () {
    // What a node writes into an order's `source` tag, and what the web
    // client's address carries percent-encoded in its `link` parameter.
    const node =
        '0000cc02101ec29eea9ce623258752b9d7da66c27845ed26846dd0b0fc736b40';
    const link =
        'mostro:$_id?relays=wss://relay.mostro.network,wss://nos.lol'
        '&mostro=$node';
    const encoded =
        'mostro%3A$_id%3Frelays%3Dwss%3A%2F%2Frelay.mostro.network%2C'
        'wss%3A%2F%2Fnos.lol%26mostro%3D$node';
    const address = 'https://mostro.network/app/#/order_link?link=$encoded';

    test('reads the link the web client was opened on', () {
      expect(orderLinkFromAppUrl(address), link);
    });

    test('keeps a link whole past its own separators', () {
      // The link carries a query of its own: its `&` arrives as `%26` and
      // must stay inside the value, not cut it short before `mostro=`.
      final read = orderLinkFromAppUrl(address);
      expect(read, contains('&mostro='));
      expect(read, contains('relay.mostro.network,wss://nos.lol'));
    });

    test('survives the round trip the browser address makes through Uri', () {
      // Production reads `Uri.base.toString()`, not the raw href. Percent
      // escapes of reserved characters must come back out as they went in:
      // a `%26` normalized to `&` would end the `link` parameter early and
      // hand the core a link without its node.
      expect(orderLinkFromAppUrl(Uri.parse(address).toString()), link);
    });

    test('leaves every other address alone', () {
      for (final url in [
        'https://mostro.network/app/',
        'https://mostro.network/app/#/',
        'https://mostro.network/app/#/order_book',
        'https://mostro.network/app/#/order_link',
        'https://mostro.network/app/#/order_link?link=',
        'https://mostro.network/app/#/order_link?link=nostr%3Anpub1abc',
        'https://mostro.network/app/#/order_link?order=$encoded',
      ]) {
        expect(orderLinkFromAppUrl(url), isNull, reason: url);
      }
    });
  });
}
