import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:mostro/core/app_routes.dart';
import 'package:mostro/features/notifications/services/push_notification_service.dart';

/// The app's route tree with every screen replaced by a line naming its
/// location, so the stack a navigation builds can be driven without the
/// real screens.
List<RouteBase> _placeholders(List<RouteBase> routes) => [
  for (final route in routes.cast<GoRoute>())
    GoRoute(
      path: route.path,
      builder:
          (_, state) => Scaffold(body: Text('at ${state.matchedLocation}')),
      routes: _placeholders(route.routes),
    ),
];

/// Every location the app routes to, with a sample id in each parameter.
List<String> _locations(List<RouteBase> routes, [String parent = '']) => [
  for (final route in routes.cast<GoRoute>()) ...[
    _join(parent, route.path),
    ..._locations(route.routes, _join(parent, route.path)),
  ],
];

String _join(String parent, String path) {
  final joined =
      path.startsWith('/')
          ? path
          : parent.endsWith('/')
          ? '$parent$path'
          : '$parent/$path';
  return joined.replaceAllMapped(RegExp(r':(\w+)'), (m) => 'sample-${m[1]}');
}

/// Mounts the app's route tree and records each time back left the app.
Future<(GoRouter, List<String>)> _pump(WidgetTester tester) async {
  final router = GoRouter(
    initialLocation: AppRoute.home,
    routes: _placeholders(appRouter.configuration.routes),
  );
  addTearDown(router.dispose);
  final exits = <String>[];
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    SystemChannels.platform,
    (call) async {
      if (call.method == 'SystemNavigator.pop') exits.add(call.method);
      return null;
    },
  );
  addTearDown(
    () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      null,
    ),
  );
  await tester.pumpWidget(MaterialApp.router(routerConfig: router));
  await tester.pumpAndSettle();
  return (router, exits);
}

/// The system back button (Android's back gesture).
Future<void> _back(WidgetTester tester) async {
  await tester.binding.handlePopRoute();
  await tester.pumpAndSettle();
}

const _firstRun = {AppRoute.walkthrough, AppRoute.chooseNode};

void main() {
  testWidgets('back from every screen ends on Book before it leaves the app', (
    tester,
  ) async {
    final (router, exits) = await _pump(tester);
    final screens = [
      for (final location in _locations(appRouter.configuration.routes))
        if (location != AppRoute.home && !_firstRun.contains(location))
          location,
    ];
    expect(screens, contains(AppRoute.tradeDetailPath('sample-orderId')));

    for (final location in screens) {
      // Reached by `go`, as after taking an order or from a link: whatever
      // the screen, Book has to be somewhere under it.
      router.go(location);
      await tester.pumpAndSettle();
      for (var step = 0; step < 3 && exits.isEmpty; step++) {
        if (find.text('at ${AppRoute.home}').evaluate().isNotEmpty) break;
        await _back(tester);
      }

      expect(exits, isEmpty, reason: 'back from $location left the app');
      expect(
        find.text('at ${AppRoute.home}'),
        findsOneWidget,
        reason: 'back from $location',
      );
    }
  });

  testWidgets('back from Book leaves the app', (tester) async {
    final (_, exits) = await _pump(tester);

    await _back(tester);

    expect(exits, hasLength(1));
  });

  testWidgets('a notification tap opens Notifications with Book under it, '
      'whatever was open', (tester) async {
    final (router, exits) = await _pump(tester);
    router.go(AppRoute.chatRoomPath('o1'));
    await tester.pumpAndSettle();
    router.push(AppRoute.tradeDetailPath('o1'));
    await tester.pumpAndSettle();

    // Two pushes arrived while the user was away.
    openNotificationsOnTap(router);
    await tester.pumpAndSettle();
    openNotificationsOnTap(router);
    await tester.pumpAndSettle();
    expect(find.text('at ${AppRoute.notifications}'), findsOneWidget);
    await _back(tester);

    expect(find.text('at ${AppRoute.home}'), findsOneWidget);
    expect(exits, isEmpty);
  });

  test('every screen keeps its URL', () {
    // Links from outside the app (the web build, the messaging worker's
    // `#/notifications`) name these paths.
    for (final location in [
      AppRoute.home,
      AppRoute.orderBook,
      AppRoute.chatList,
      AppRoute.notifications,
      AppRoute.tradeDetailPath('o1'),
      AppRoute.chatRoomPath('o1'),
      AppRoute.disputeChatPath('d1'),
      AppRoute.aboutTechnical,
    ]) {
      final match = appRouter.configuration.findMatch(Uri.parse(location));
      expect(match.isError, isFalse, reason: location);
      expect(match.last.matchedLocation, location);
    }
  });
}
