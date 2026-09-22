import SwiftUI

/// The tester's appearance switch for a standalone rehearsal, in the intro
/// and mailbox mastheads. It drives the app's real theme preference — the
/// same one `\` flips and Settings shows — so what it sets is what the live
/// inbox comes back in.
struct DemoAppearancePicker: View {
    @State private var prefs = Prefs.shared

    var body: some View {
        HStack(spacing: 2) {
            option(.light, title: "Light", symbol: "sun.max.fill")
            option(.dark, title: "Dark", symbol: "moon.fill")
        }
        .padding(3)
        .background(Palette.ink.opacity(0.06), in: Capsule())
        .overlay { Capsule().strokeBorder(Palette.hairline, lineWidth: 0.5) }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Demo appearance")
    }

    private func option(_ theme: ThemeChoice, title: String, symbol: String) -> some View {
        Button { prefs.theme = theme } label: {
            Label(title, systemImage: symbol)
                .font(.system(size: 11, weight: .medium))
                .padding(.horizontal, 9)
                .padding(.vertical, 5)
                .foregroundStyle(prefs.theme == theme ? Palette.ink : Palette.inkDim)
                .background {
                    if prefs.theme == theme {
                        Capsule().fill(Palette.readerBackground)
                            .shadow(color: .black.opacity(0.08), radius: 2, y: 1)
                    }
                }
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .help("Use \(title.lowercased()) appearance")
        .accessibilityAddTraits(prefs.theme == theme ? .isSelected : [])
    }
}
