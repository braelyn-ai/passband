import SwiftUI

/// Fictional newsletter identities, drawn locally in the same thumbnail and
/// avatar positions used by real mail. No URL resolution or image decoding.
enum RehearsalNewsletterBrand {
    case cats, haightssion, federal, brightly, rainforest, exfed

    static func matching(_ sender: String) -> Self? {
        guard RehearsalMode.isEnabled else { return nil }
        let address = SenderID.parse(sender).addr.lowercased()
        switch address {
        case "tracking@exfed.example": return .exfed
        case "orders@rainforest.example": return .rainforest
        case "updates@brightly.example": return .brightly
        case "newsletter@catsweekly.example": return .cats
        case "program@haightssion.example": return .haightssion
        case "edition@federaloverstatement.example": return .federal
        default: return nil
        }
    }

    var name: String {
        switch self {
        case .exfed: "ExFed"
        case .rainforest: "Rainforest"
        case .brightly: "Brightly"
        case .cats: "Cats Weekly"
        case .haightssion: "Haightssion"
        case .federal: "The Federal Overstatement"
        }
    }
}

struct RehearsalNewsletterLogo: View {
    let brand: RehearsalNewsletterBrand
    let size: CGFloat
    /// False drops a WHITE backdrop so the mark sits straight on the card, the
    /// way the real carrier badges do once their white square is cleared. A
    /// coloured tile is the brand itself and stays either way.
    var tile: Bool = true

    var body: some View {
        ZStack {
            switch brand {
            case .exfed:
                if tile { Color.white }
                VStack(spacing: -size * 0.06) {
                    Text("Ex").foregroundStyle(Color(hex: 0x51258A))
                    Text("Fed").foregroundStyle(Color(hex: 0xF46A20))
                }
                .font(.system(size: size * 0.40, weight: .black))
                .tracking(-size * 0.035)
            case .rainforest:
                Color(hex: 0x232F3E)
                Text("r")
                    .font(.system(size: size * 0.79, weight: .bold, design: .rounded))
                    .foregroundStyle(.white)
                    .offset(y: -size * 0.10)
                RainforestSmile()
                    .stroke(Color(hex: 0xFF9900), style: StrokeStyle(lineWidth: max(1.2, size * 0.065), lineCap: .round, lineJoin: .round))
            case .brightly:
                Color(hex: 0x171717)
                BrightlyMark()
                    .fill(Color(hex: 0xFFE500))
                    .padding(size * 0.12)
            case .cats:
                Color(hex: 0xF5EFDF)
                VStack(spacing: size * 0.025) {
                    Image(systemName: "cat.fill")
                        .font(.system(size: size * (size > 32 ? 0.43 : 0.62), weight: .medium))
                    if size > 32 {
                        Text("CW")
                            .font(.system(size: size * 0.20, weight: .bold, design: .serif))
                            .tracking(size * 0.035)
                    }
                }
                .foregroundStyle(Color(hex: 0x293B30))
            case .haightssion:
                Color(hex: 0x171717)
                HaightssionMark()
                    .fill(Color(hex: 0xD9FF43))
                    .padding(size * 0.12)
            case .federal:
                Color(hex: 0xFFFDF6)
                VStack(spacing: size * 0.05) {
                    Rectangle().frame(height: max(1, size * 0.025))
                    Text("FO")
                        .font(.system(size: size * 0.43, weight: .bold, design: .serif))
                        .tracking(-size * 0.025)
                    VStack(spacing: max(1, size * 0.025)) {
                        Rectangle().frame(height: max(1, size * 0.025))
                        Rectangle().frame(height: max(1, size * 0.015))
                    }
                }
                .foregroundStyle(Color(hex: 0x181818))
                .padding(.horizontal, size * 0.13)
                .padding(.vertical, size * 0.16)
            }
        }
        .frame(width: size, height: size)
        .clipShape(RoundedRectangle(cornerRadius: size * 0.15, style: .continuous))
        .overlay {
            if tile {
                RoundedRectangle(cornerRadius: size * 0.15, style: .continuous)
                    .strokeBorder(.black.opacity(0.08), lineWidth: 0.5)
            }
        }
        .accessibilityLabel("\(brand.name) logo")
    }
}

/// An original angular club mark: three interlocking cuts with hard corners.
private struct HaightssionMark: Shape {
    func path(in rect: CGRect) -> Path {
        let pieces: [[CGPoint]] = [
            [CGPoint(x: 0.08, y: 0.08), CGPoint(x: 0.62, y: 0.08),
             CGPoint(x: 0.42, y: 0.28), CGPoint(x: 0.30, y: 0.28),
             CGPoint(x: 0.30, y: 0.70), CGPoint(x: 0.08, y: 0.92)],
            [CGPoint(x: 0.92, y: 0.08), CGPoint(x: 0.92, y: 0.92),
             CGPoint(x: 0.38, y: 0.92), CGPoint(x: 0.58, y: 0.72),
             CGPoint(x: 0.70, y: 0.72), CGPoint(x: 0.70, y: 0.30)],
            [CGPoint(x: 0.30, y: 0.49), CGPoint(x: 0.49, y: 0.30),
             CGPoint(x: 0.70, y: 0.30), CGPoint(x: 0.70, y: 0.51),
             CGPoint(x: 0.51, y: 0.70), CGPoint(x: 0.30, y: 0.70)],
        ]
        var path = Path()
        for piece in pieces {
            let points = piece.map { CGPoint(x: rect.minX + $0.x * rect.width, y: rect.minY + $0.y * rect.height) }
            path.addLines(points)
            path.closeSubpath()
        }
        return path
    }
}

/// Matches the oversized burst in the fictional service notice.
private struct BrightlyMark: Shape {
    func path(in rect: CGRect) -> Path {
        let coordinates: [(CGFloat, CGFloat)] = [
            (50, 0), (58, 20), (75, 7), (73, 29), (93, 25), (80, 43),
            (100, 50), (80, 58), (93, 75), (71, 73), (75, 93), (57, 80),
            (50, 100), (42, 80), (25, 93), (27, 71), (7, 75), (20, 57),
            (0, 50), (20, 42), (7, 25), (29, 27), (25, 7), (43, 20),
        ]
        var path = Path()
        path.addLines(coordinates.map { CGPoint(x: rect.minX + $0.0 * rect.width / 100,
                                                y: rect.minY + $0.1 * rect.height / 100) })
        path.closeSubpath()
        return path
    }
}

/// The fictional retailer's orange delivery swoosh.
private struct RainforestSmile: Shape {
    func path(in rect: CGRect) -> Path {
        func point(_ x: CGFloat, _ y: CGFloat) -> CGPoint {
            CGPoint(x: rect.minX + x * rect.width, y: rect.minY + y * rect.height)
        }
        var path = Path()
        path.move(to: point(0.20, 0.73))
        path.addQuadCurve(to: point(0.80, 0.69), control: point(0.50, 0.95))
        path.move(to: point(0.65, 0.69))
        path.addLine(to: point(0.81, 0.67))
        path.addLine(to: point(0.77, 0.82))
        return path
    }
}
