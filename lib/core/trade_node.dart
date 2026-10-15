/// Whether a trade whose row names [tradeNode] lives on a node other than
/// [activeNode] (#811). A row that does not know its node (empty) counts as
/// the active node's, as Rust treats it (#810).
bool isOnOtherNode({required String tradeNode, required String activeNode}) =>
    tradeNode.isNotEmpty && tradeNode.toLowerCase() != activeNode.toLowerCase();
