import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:mostro/core/app_theme.dart';
import 'package:mostro/core/create_order_palette.dart';
import 'package:mostro/core/order_detail_palette.dart';
import 'package:mostro/l10n/app_localizations.dart';
import 'package:mostro/shared/widgets/mostro_modal.dart';
import 'package:mostro/features/settings/providers/settings_provider.dart';
import 'package:mostro/src/rust/api/invoice.dart' as invoice_api;
import 'package:mostro/src/rust/api/settings.dart' as settings_api;
import 'package:mostro/src/rust/api/types.dart'
    show
        PaymentDestination_LightningAddress,
        TakePayout,
        TakePayout_Address,
        TakePayout_AskForInvoice;

/// Whether a typed string is a Lightning address, as the Rust core judges it
/// (the same rule `take_order` applies); a seam for tests.
final lightningAddressCheckProvider = Provider<Future<bool> Function(String)>(
  (ref) =>
      (input) async =>
          await invoice_api.classifyPaymentDestination(input: input)
              is PaymentDestination_LightningAddress,
);

/// The default Lightning address a take sends: the core's copy, which only
/// ever holds an address the core accepted. Settings can still hold one the
/// core refused (saved before the dialog asked it), and naming that one
/// would promise a destination the take never sends (PR #824 review).
/// Re-read when the saved address changes.
final takeDefaultAddressProvider = FutureProvider.autoDispose<String?>((
  ref,
) async {
  ref.watch(settingsProvider.select((s) => s.defaultLightningAddress));
  try {
    final address = (await settings_api.getSettings()).defaultLightningAddress;
    return address == null || address.isEmpty ? null : address;
  } catch (e) {
    debugPrint('[TakePayoutSheet] default address unreadable: $e');
    return null;
  }
});

/// Asks where a buyer's sats go for this trade (#823): the default Lightning
/// address, another one, or an invoice added after the take. Null when the
/// sheet is dismissed, which keeps the choice as it was.
Future<TakePayout?> showTakePayoutSheet(
  BuildContext context, {
  required String defaultAddress,
  TakePayout? current,
}) => showMostroSheet<TakePayout>(
  context: context,
  builder:
      (_) => TakePayoutSheet(defaultAddress: defaultAddress, current: current),
);

enum _Choice { defaultAddress, otherAddress, invoice }

/// The sheet of [showTakePayoutSheet].
class TakePayoutSheet extends ConsumerStatefulWidget {
  const TakePayoutSheet({
    super.key,
    required this.defaultAddress,
    this.current,
  });

  /// The default Lightning address from Settings.
  final String defaultAddress;

  /// The choice made so far; null is the default address.
  final TakePayout? current;

  @override
  ConsumerState<TakePayoutSheet> createState() => _TakePayoutSheetState();
}

class _TakePayoutSheetState extends ConsumerState<TakePayoutSheet> {
  late _Choice _choice = switch (widget.current) {
    TakePayout_Address() => _Choice.otherAddress,
    TakePayout_AskForInvoice() => _Choice.invoice,
    _ => _Choice.defaultAddress,
  };
  late final TextEditingController _address = TextEditingController(
    text: switch (widget.current) {
      TakePayout_Address(:final address) => address,
      _ => '',
    },
  );
  String? _error;
  bool _checking = false;

  @override
  void dispose() {
    _address.dispose();
    super.dispose();
  }

  Future<void> _apply() async {
    switch (_choice) {
      case _Choice.defaultAddress:
        Navigator.of(context).pop(const TakePayout.defaultAddress());
      case _Choice.invoice:
        Navigator.of(context).pop(const TakePayout.askForInvoice());
      case _Choice.otherAddress:
        final address = _address.text.trim();
        final l10n = AppLocalizations.of(context);
        setState(() => _checking = true);
        var valid = false;
        if (address.isNotEmpty) {
          try {
            valid = await ref.read(lightningAddressCheckProvider)(address);
          } catch (e) {
            debugPrint('[TakePayoutSheet] address check failed: $e');
          }
        }
        if (!mounted) return;
        // The buyer picked another option, or edited the address, while the
        // check ran: its answer is about something no longer on screen
        // (PR #824 review). The next Use checks what is.
        if (_choice != _Choice.otherAddress ||
            _address.text.trim() != address) {
          setState(() => _checking = false);
          return;
        }
        if (!valid) {
          setState(() {
            _checking = false;
            _error = l10n.invalidLightningAddressFormat;
          });
          return;
        }
        Navigator.of(context).pop(TakePayout.address(address: address));
    }
  }

  void _select(_Choice choice) => setState(() {
    _choice = choice;
    _error = null;
  });

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context);
    final book = OrderBookPalette.of(context);
    final create = CreateOrderPalette.of(context);
    final danger = OrderDetailPalette.of(context).danger;
    return MostroSheet(
      title: l10n.takeOrderPayoutSheetTitle,
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _ChoiceRow(
            selected: _choice == _Choice.defaultAddress,
            title: l10n.takeOrderPayoutSheetDefault,
            subtitle: widget.defaultAddress,
            onTap: () => _select(_Choice.defaultAddress),
          ),
          _ChoiceRow(
            selected: _choice == _Choice.otherAddress,
            title: l10n.takeOrderPayoutSheetOther,
            onTap: () => _select(_Choice.otherAddress),
          ),
          if (_choice == _Choice.otherAddress)
            Padding(
              padding: const EdgeInsets.fromLTRB(32, 0, 0, 8),
              child: TextField(
                controller: _address,
                autofocus: true,
                autocorrect: false,
                enableSuggestions: false,
                keyboardType: TextInputType.emailAddress,
                textInputAction: TextInputAction.done,
                style: TextStyle(fontSize: 14, color: book.textPrimary),
                // Every state set here (DS-CMP-19): left to the theme, the
                // field would paint v1's filled underline.
                decoration: InputDecoration(
                  isDense: true,
                  filled: false,
                  hintText: l10n.lightningAddressHintText,
                  hintStyle: TextStyle(fontSize: 14, color: book.textFaint),
                  errorText: _error,
                  errorStyle: TextStyle(fontSize: 12, color: danger),
                  enabledBorder: UnderlineInputBorder(
                    borderSide: BorderSide(color: create.fieldUnderline),
                  ),
                  focusedBorder: UnderlineInputBorder(
                    borderSide: BorderSide(
                      color: create.fieldUnderlineFocus,
                      width: 1.5,
                    ),
                  ),
                  errorBorder: UnderlineInputBorder(
                    borderSide: BorderSide(color: danger),
                  ),
                  focusedErrorBorder: UnderlineInputBorder(
                    borderSide: BorderSide(color: danger, width: 1.5),
                  ),
                ),
                onChanged: (_) {
                  if (_error != null) setState(() => _error = null);
                },
                onSubmitted: (_) => _apply(),
              ),
            ),
          _ChoiceRow(
            selected: _choice == _Choice.invoice,
            title: l10n.takeOrderPayoutSheetInvoice,
            onTap: () => _select(_Choice.invoice),
          ),
        ],
      ),
      primary: ModalAction(
        label: l10n.takeOrderPayoutSheetApply,
        onPressed: _checking ? null : _apply,
        busy: _checking,
      ),
    );
  }
}

/// One option of the sheet: a radio mark, its title and an optional line
/// under it. The whole row is the target (DS-CMP-6).
class _ChoiceRow extends StatelessWidget {
  const _ChoiceRow({
    required this.selected,
    required this.title,
    required this.onTap,
    this.subtitle,
  });

  final bool selected;
  final String title;
  final String? subtitle;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final book = OrderBookPalette.of(context);
    final subtitle = this.subtitle;
    return Semantics(
      button: true,
      selected: selected,
      inMutuallyExclusiveGroup: true,
      child: Material(
        type: MaterialType.transparency,
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(12),
          child: ConstrainedBox(
            constraints: const BoxConstraints(minHeight: 48),
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 8),
              child: Row(
                children: [
                  Icon(
                    selected
                        ? Icons.radio_button_checked
                        : Icons.radio_button_unchecked,
                    size: 20,
                    color: selected ? book.limeIcon : book.textTertiary,
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          title,
                          style: TextStyle(
                            fontSize: 14,
                            fontWeight: FontWeight.w500,
                            color: book.textPrimary,
                          ),
                        ),
                        if (subtitle != null)
                          Text(
                            subtitle,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                              fontSize: 12,
                              color: book.textSecondary,
                            ),
                          ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
