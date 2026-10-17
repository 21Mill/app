import 'dart:async';

import 'package:app_links/app_links.dart';
import 'package:clock/clock.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:mostro/core/app_routes.dart';
import 'package:mostro/features/deep_links/models/order_link_rules.dart';
import 'package:mostro/features/walkthrough/providers/first_run_provider.dart';

/// Hands the `mostro:` links the OS opens the app with to the order link
/// screen (`OrderLinkScreen`), which does everything else.
///
/// A link comes from outside the app, so it is a `go`, as a notification tap
/// is (FR-057a): the screen opens on Book, not over the screens the user had
/// open, and back ends there.
///
/// Wraps the app root, like `TradeActionListener`, and listens for the whole
/// session. The link that launched the app arrives here too, as the first
/// event: `app_links` keeps it until someone listens.
///
/// A link that arrives during the first run waits for it to end — the router
/// would send the order link screen to the walkthrough and lose it. The same
/// link twice within [duplicateWindow] opens once: a platform may hand the
/// launch link over again, as v1 saw.
class DeepLinkListener extends ConsumerStatefulWidget {
  const DeepLinkListener({
    super.key,
    required this.child,
    this.links,
    this.open,
    this.launchLink,
  });

  final Widget child;

  /// Test seam — production listens to the OS through `app_links`. The web
  /// gets none: a page cannot be the handler of a `mostro:` link.
  final Stream<String>? links;

  /// Test seam — production goes to the destination on the global
  /// [appRouter].
  final void Function(String destination)? open;

  /// The order link the app was opened with, when it did not come from the OS.
  ///
  /// The web has no OS link stream ([links] is empty there): a page cannot be
  /// the handler of a `mostro:` link, so the link reaches the app as its own
  /// address. Whoever starts the app reads it before the router's first
  /// decision and hands it over here.
  final String? launchLink;

  static const duplicateWindow = Duration(seconds: 2);

  @override
  ConsumerState<DeepLinkListener> createState() => _DeepLinkListenerState();
}

class _DeepLinkListenerState extends ConsumerState<DeepLinkListener> {
  StreamSubscription<String>? _subscription;

  /// A link that arrived before the first run ended.
  String? _waiting;

  /// Whether the first run's state has been known once. Until it is, nothing
  /// says yet whether [DeepLinkListener.launchLink] has to be kept.
  bool _firstRunSettled = false;

  String? _lastLink;
  DateTime? _lastAt;

  @override
  void initState() {
    super.initState();
    // After the first frame, so the router has its first location before a
    // link moves it.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      _subscription = (widget.links ?? _osLinks()).listen(
        _onLink,
        onError: (Object e) => debugPrint('[deep-link] link stream failed: $e'),
      );
      _settleFirstRun(ref.read(firstRunProvider));
    });
  }

  /// Decides, the first time the first run's state is known, whether the link
  /// the app was opened with has to be kept.
  ///
  /// Only an unfinished first run takes that address away: the router sends
  /// the order link screen to the walkthrough and drops the link, which is
  /// what [_waiting] exists for. A first run already complete served the
  /// address itself, so opening it again would only look the order up twice.
  void _settleFirstRun(AsyncValue<bool> firstRun) {
    final done = firstRun.valueOrNull;
    if (done == null || _firstRunSettled) return;
    _firstRunSettled = true;
    final link = widget.launchLink?.trim();
    if (done || link == null || !isOrderLink(link)) return;
    _waiting = link;
  }

  static Stream<String> _osLinks() =>
      kIsWeb ? const Stream.empty() : AppLinks().stringLinkStream;

  @override
  void dispose() {
    _subscription?.cancel();
    super.dispose();
  }

  void _onLink(String raw) {
    final link = raw.trim();
    // Only `mostro:` is registered, but a platform may still pass on
    // something else (the web's page address, a desktop argument).
    if (!isOrderLink(link)) return;

    final now = clock.now();
    final lastAt = _lastAt;
    if (link == _lastLink &&
        lastAt != null &&
        now.difference(lastAt) < DeepLinkListener.duplicateWindow) {
      return;
    }
    _lastLink = link;
    _lastAt = now;

    if (ref.read(firstRunProvider).valueOrNull != true) {
      _waiting = link;
      return;
    }
    _openLink(link);
  }

  void _openLink(String link) {
    final destination = AppRoute.orderLinkPath(link);
    final open =
        widget.open ?? (String destination) => appRouter.go(destination);
    try {
      open(destination);
    } catch (e) {
      debugPrint('[deep-link] could not open the order link screen: $e');
    }
  }

  @override
  Widget build(BuildContext context) {
    ref.listen<AsyncValue<bool>>(firstRunProvider, (_, next) {
      _settleFirstRun(next);
      final link = _waiting;
      if (link == null || next.valueOrNull != true) return;
      _waiting = null;
      // On the next frame, not now: Riverpod notifies while the screen that
      // ended the first run is still in its own handler, and the node choice
      // goes home right after awaiting `markFirstRunComplete` — navigating
      // here would be overwritten by that `go` a line later.
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _openLink(link);
      });
    });
    return widget.child;
  }
}
