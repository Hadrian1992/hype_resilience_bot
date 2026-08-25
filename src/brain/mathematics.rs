use rust_decimal::Decimal;

/// Compute orderbook depth: sum bids within pct_threshold below current price.
pub fn compute_bid_depth_usd(bids: &[(f64, f64)], current_price: f64, pct_threshold: f64) -> f64 {
    let limit = current_price * (1.0 - pct_threshold);
    bids.iter()
        .filter(|&&(px, _sz)| px >= limit)
        .map(|&(_px, sz)| sz)
        .sum()
}
