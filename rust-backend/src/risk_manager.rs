use serde::{Deserialize, Serialize};

/// Kết quả tính toán khối lượng lệnh (Position Sizing Result) cho XAU/USD
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionSizeResult {
    /// Khối lượng vào lệnh khuyến nghị (Standard Lots, e.g. 0.10, 0.25)
    pub lot_size: f64,
    /// Số tiền rủi ro thực tế sau khi chuẩn hóa lot bằng USD ($)
    pub risk_amount_usd: f64,
    /// Lợi nhuận dự kiến tại Take Profit 1 ($)
    pub potential_profit_tp1_usd: f64,
    /// Lợi nhuận dự kiến tại Take Profit 2 ($)
    pub potential_profit_tp2_usd: f64,
    /// Tỷ lệ ký quỹ ước tính (Margin Requirement) với đòn bẩy chuẩn (e.g. 1:100)
    pub estimated_margin_usd: f64,
    /// Ghi chú quản lý vốn an toàn
    pub risk_assessment: String,
}

pub struct PositionSizer;

impl PositionSizer {
    /// Quy cách hợp đồng Vàng tiêu chuẩn quốc tế:
    /// 1.0 Standard Lot = 100 Ounces vàng
    /// Mỗi 1.0$ biến động giá trên 1.0 lot = 100$ USD lợi nhuận hoặc thua lỗ
    pub const OUNCES_PER_LOT: f64 = 100.0;
    pub const MIN_LOT: f64 = 0.01; // Micro lot tối thiểu của sàn
    pub const MAX_LOT: f64 = 50.0; // Giới hạn an toàn chống cháy tài khoản

    /// Hàm tự động tính khối lượng giao dịch (Lot Size) chuẩn xác
    ///
    /// Công thức:
    /// 1. Max Risk Amount ($) = Account Balance * (Risk % / 100)
    /// 2. SL Dollar Value per 1.0 lot = SL Distance (points) * 100 (Ounces)
    /// 3. Raw Lot = Max Risk Amount / SL Dollar Value per 1.0 lot
    /// 4. Làm tròn xuống 2 chữ số thập phân (floor to 0.01) để không bao giờ vượt quá rủi ro
    /// 5. Risk Amount thực tế = Lot đã chuẩn hóa * SL Dollar Value per 1.0 lot
    pub fn calculate_lot_size(
        account_balance: f64,
        risk_percentage: f64,
        sl_distance_points: f64,
        reward_tp1_points: f64,
        reward_tp2_points: f64,
        current_gold_price: f64,
        leverage: f64, // e.g. 100.0 hoặc 500.0
    ) -> PositionSizeResult {
        // Kiểm tra an toàn đầu vào
        let balance = account_balance.max(100.0);
        let risk_pct = risk_percentage.clamp(0.1, 5.0); // Giới hạn tối đa 5% rủi ro
        let sl_points = sl_distance_points.max(1.0);

        // 1. Số tiền rủi ro tối đa cho phép (trước khi chuẩn hóa lot)
        let max_risk_usd = (balance * (risk_pct / 100.0) * 100.0).round() / 100.0;

        // 2. Giá trị rủi ro trên 1.0 lot chuẩn khi dính Stop Loss
        let loss_per_one_lot = sl_points * Self::OUNCES_PER_LOT;

        // 3. Tính toán Lot Size
        let raw_lot = max_risk_usd / loss_per_one_lot;

        // 4. Chuẩn hóa và làm tròn xuống bước giá 0.01 lot
        let mut normalized_lot = (raw_lot * 100.0).floor() / 100.0;
        normalized_lot = normalized_lot.clamp(Self::MIN_LOT, Self::MAX_LOT);

        // 5. Rủi ro thực tế sau khi chuẩn hóa lot (thường <= max_risk_usd; MIN_LOT
        //    có thể đẩy rủi ro lên cao hơn nếu tài khoản quá nhỏ so với SL).
        let risk_amount_usd = (normalized_lot * loss_per_one_lot * 100.0).round() / 100.0;

        // 6. Tính toán các mốc lợi nhuận kỳ vọng theo số Lot thực tế
        let profit_tp1_usd =
            (normalized_lot * reward_tp1_points * Self::OUNCES_PER_LOT * 100.0).round() / 100.0;
        let profit_tp2_usd =
            (normalized_lot * reward_tp2_points * Self::OUNCES_PER_LOT * 100.0).round() / 100.0;

        // 7. Tính toán tiền ký quỹ (Margin Required) = (Lot * 100 oz * Price) / Leverage
        let lev = leverage.max(1.0);
        let estimated_margin_usd =
            ((normalized_lot * Self::OUNCES_PER_LOT * current_gold_price / lev) * 100.0).round()
                / 100.0;

        let risk_assessment = format!(
            "Tài khoản ${:.2} | Rủi ro cấu hình {:.1}% | Rủi ro thực tế ${:.2} | Khoảng cách SL {:.1}$ => Khối lượng khuyến nghị: {:.2} Lot. Nếu chạm TP1 lãi +${:.2}, chạm TP2 lãi +${:.2}.",
            balance, risk_pct, risk_amount_usd, sl_points, normalized_lot, profit_tp1_usd, profit_tp2_usd
        );

        PositionSizeResult {
            lot_size: normalized_lot,
            risk_amount_usd,
            potential_profit_tp1_usd: profit_tp1_usd,
            potential_profit_tp2_usd: profit_tp2_usd,
            estimated_margin_usd,
            risk_assessment,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lot_calculation() {
        // Tài khoản $10,000, rủi ro 1% ($100), SL 10.0$
        // Mỗi 1.0 lot lỗ 10.0 * 100 = $1,000
        // Khối lượng lý tưởng = 100 / 1000 = 0.10 Lot
        let result =
            PositionSizer::calculate_lot_size(10000.0, 1.0, 10.0, 20.0, 30.0, 2680.0, 100.0);

        assert_eq!(result.lot_size, 0.10);
        assert_eq!(result.risk_amount_usd, 100.0);
        assert_eq!(result.potential_profit_tp1_usd, 200.0); // R:R 1:2
        assert_eq!(result.potential_profit_tp2_usd, 300.0); // R:R 1:3
    }

    #[test]
    fn test_lot_rounding_reports_actual_risk_below_max() {
        // Tài khoản $10,000, rủi ro 1% => max risk $100, SL 13.0$
        // Mỗi 1.0 lot lỗ 13.0 * 100 = $1,300
        // Raw lot = 100 / 1300 = 0.0769 => làm tròn xuống 0.07 lot
        // Rủi ro thực tế = 0.07 * 1300 = $91.00 (< $100)
        let result =
            PositionSizer::calculate_lot_size(10000.0, 1.0, 13.0, 26.0, 39.0, 2680.0, 100.0);

        assert_eq!(result.lot_size, 0.07);
        assert_eq!(result.risk_amount_usd, 91.0);
        assert!(result.risk_amount_usd < 100.0);
        // Lợi nhuận vẫn tính theo lot đã chuẩn hóa.
        assert_eq!(result.potential_profit_tp1_usd, 182.0);
        assert_eq!(result.potential_profit_tp2_usd, 273.0);
        assert!(result.risk_assessment.contains("$91.00"));
    }
}
