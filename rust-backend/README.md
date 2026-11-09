# 🚀 Gold Signal Bot - High-Performance Rust Trading Engine

Hệ thống bot phát hiện và bắn tín hiệu giao dịch Vàng (XAU/USD) tự động hiệu năng cao viết bằng **Rust (Tokio + Axum + Reqwest)** kết nối trực tiếp với **Telegram Bot API**.

---

## 🎯 Cốt lõi Thuật toán (Business Logic)
1. **Dynamic Support / Resistance (S/R):** Quét các đỉnh và đáy sóng cục bộ (Swing Highs & Lows), tự động gom cụm (Clustering) các mức giá có độ lệch $\le 2.5\$$ để xác định vùng thanh khoản tổ chức (Liquidity Zones).
2. **Bộ lọc Nến Rút chân (Pinbar / Rejection):**
   - **Bullish Pinbar (Buy):** Nến test vùng hỗ trợ, có râu dưới dài $\ge 2.0\times$ thân nến và chiếm $> 50\%$ biên độ nến; râu trên ngắn $< 25\%$.
   - **Bearish Pinbar (Sell):** Nến test vùng kháng cự, có râu trên dài $\ge 2.0\times$ thân nến và chiếm $> 50\%$ biên độ nến; râu dưới ngắn $< 25\%$.
3. **Bộ lọc Volume Hấp Thụ (Smart Money Absorption):**
   - Tính đường trung bình động Volume 20 chu kỳ ($MA_{20}$).
   - Yêu cầu Volume nến tín hiệu phải đạt tối thiểu $\ge 1.5\times MA_{20}$ để loại bỏ bẫy giá khối lượng thấp (Low-volume fakeouts).
4. **Tự Động Tính Toán R:R (Risk-to-Reward $\ge 1:2$):**
   - Đệm Stop Loss an toàn: cách cản $8.0 - 10.0\$$ đối với giá vàng.
   - $TP_1$: Tính theo tỷ lệ chuẩn $1:2$ so với khoảng cắt lỗ.
   - $TP_2$: Mở rộng $1:3$ hoặc vùng cản tiếp theo.
5. **Cảnh Báo Telegram:** Định dạng HTML rõ ràng, chuyên nghiệp với chi tiết Entry, TP1, TP2, SL, R:R và giải thích Volume.

---

## 📁 Cấu trúc Thư mục Dự án Rust
```text
rust-backend/
├── Cargo.toml            # Khai báo dependency: Tokio, Axum, Reqwest, Serde, Tracing
├── .env.example          # Mẫu biến môi trường
├── README.md             # Tài liệu kiến trúc và hướng dẫn cài đặt
└── src/
    ├── main.rs           # Khởi tạo runtime Tokio, Background Scanner, Axum Server
    ├── models.rs         # Định nghĩa Struct: Candle, Signal, BotConfig, S/R Zones
    ├── analyzer.rs       # Trái tim thuật toán: S/R detector, Pinbar, Volume MA, R:R calculator
    ├── telegram.rs       # Client Telegram Bot API với định dạng tin nhắn HTML
    └── api.rs            # Axum REST endpoints & Webhook nhận nến từ TradingView / MT4/MT5
```

---

## ⚡ Hướng dẫn Cài đặt & Chạy Rust Engine

### 1. Yêu cầu Hệ thống
- Đã cài đặt [Rust & Cargo](https://rustup.rs/) (khuyến nghị phiên bản $\ge 1.75$)

### 2. Thiết lập Biến môi trường
Tạo file `.env` trong thư mục `rust-backend/`:
```bash
TELEGRAM_BOT_TOKEN="your_bot_father_token_here"
TELEGRAM_CHAT_ID="your_telegram_chat_or_channel_id"
PORT=8080
```

### 3. Biên dịch và Khởi chạy
```bash
# Di chuyển vào thư mục backend
cd rust-backend

# Chạy ở chế độ Debug
cargo run

# Hoặc biên dịch tối ưu hóa hiệu năng cao (Release mode)
cargo build --release
./target/release/gold-signal-bot
```

---

## 🔗 Tích hợp Webhook (TradingView / MetaTrader / Python)
Hệ thống mở cổng endpoint `POST http://localhost:8080/api/candles/webhook` để nhận nến mới:
```json
{
  "symbol": "XAU/USD",
  "timeframe": "M15",
  "open": 2682.10,
  "high": 2685.00,
  "low": 2671.50,
  "close": 2684.20,
  "volume": 3450.0,
  "timestamp": 1742080000000
}
```
Khi nến gửi tới thỏa mãn điều kiện, bot sẽ tự động tính toán R:R và bắn alert lên Telegram trong vòng chưa đầy $5\text{ms}$.
