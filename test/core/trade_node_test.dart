import 'package:flutter_test/flutter_test.dart';

import 'package:mostro/core/trade_node.dart';

void main() {
  const active = 'aa11';

  test('a trade of another node is on another node', () {
    expect(isOnOtherNode(tradeNode: 'bb22', activeNode: active), isTrue);
  });

  test('the active node is the same node in either case', () {
    expect(isOnOtherNode(tradeNode: 'AA11', activeNode: active), isFalse);
  });

  test('a row that does not know its node counts as the active one', () {
    // As Rust treats it (#810): refusing it would strand the trade.
    expect(isOnOtherNode(tradeNode: '', activeNode: active), isFalse);
  });
}
