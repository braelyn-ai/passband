import Foundation

/// Display readable destinations, never tracking parameters or opaque link IDs.
/// This only changes the excerpt; the original message and links stay intact.
enum SearchPreview {
    private static let urls = try! NSRegularExpression(
        pattern: #"(?:https?://|www\.|\b(?:[a-z0-9-]+\.)+[a-z]{2,24}/)[^\s<>]+"#, options: .caseInsensitive)

    static func clean(_ text: String) -> String {
        var cleaned = text
        // Work backwards so replacing one URL does not shift the remaining ranges.
        for match in urls.matches(in: text, range: NSRange(text.startIndex..., in: text)).reversed() {
            guard let range = Range(match.range, in: cleaned) else { continue }
            let raw = String(cleaned[range])
            let url = raw.trimmingCharacters(in: CharacterSet(charactersIn: ".,;!)]}…"))
            let suffix = String(raw.dropFirst(url.count))
            let address = url.lowercased().hasPrefix("http") ? url : "https://" + url
            let readable: String
            if let parts = URLComponents(string: address), let host = parts.host {
                let domain = host.hasPrefix("www.") ? String(host.dropFirst(4)) : host
                let segments = parts.path.split(separator: "/")
                var words: [String] = []
                for segment in segments {
                    // Stop at tracking/redirect IDs rather than exposing an encoded token.
                    let value = String(segment)
                    guard value.count > 1, value.count <= 48,
                          value.contains(where: { $0.isLetter }),
                          value.allSatisfy({ $0.isLetter || $0 == "-" || $0 == "_" || $0 == " " || $0 == "." }),
                          value.count <= 24 || value.contains("-") || value.contains(" ") else { break }
                    words.append(value.replacingOccurrences(of: "_", with: "-"))
                }
                readable = domain + (words.isEmpty ? "" : "/" + words.joined(separator: "/"))
            } else {
                readable = "…"
            }
            cleaned.replaceSubrange(range, with: readable + suffix)
        }
        if let footer = cleaned.range(of: "Manage subscription", options: .caseInsensitive) {
            cleaned = String(cleaned[..<footer.lowerBound])
        }
        return cleaned.replacingOccurrences(of: #"\s+"#, with: " ", options: .regularExpression)
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
