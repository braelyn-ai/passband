import Foundation

@main
struct SearchPreviewTests {
    static func main() throws {
        let prose = "Final tickets are on sale now.Manage subscription https://mail.example/long-tracking-code"
        assert(SearchPreview.clean(prose) == "Final tickets are on sale now.")
        assert(SearchPreview.clean("Read more https://www.example.com/tickets?utm_source=email&token=secret#tracking")
            == "Read more example.com/tickets")
        assert(SearchPreview.clean("https://anjuna.activehosted.com/f/255/?t=eyJhTOKEN")
            == "anjuna.activehosted.com")
        assert(SearchPreview.clean("https://example.com/AbCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz?token=x")
            == "example.com")
        assert(SearchPreview.clean("Visit www.example.com/show-tickets?ref=tracking. Then https://example.org/music#token")
            == "Visit example.com/show-tickets. Then example.org/music")
        assert(SearchPreview.clean("See https://example.com/summer%20tickets?utm_medium=email")
            == "See example.com/summer tickets")
        assert(SearchPreview.clean("anjuna.activehosted.com/f/255/?t=longToken") == "anjuna.activehosted.com")
        assert(SearchPreview.clean("Save your tickets below.") == "Save your tickets below.")
        let decoder = JSONDecoder()
        let legacy = Data(#"{"id":1,"thread_id":"t","from_addr":"a@b.co","subject":"Tickets","received_at":"2026-09-17T00:00:00Z","snippet":"Preview"}"#.utf8)
        let old = try decoder.decode(SearchHit.self, from: legacy)
        assert(old.is_done == nil && old.snippet_matches == nil, "Old status is unknown, not unfinished")
        let current = Data(#"{"id":1,"thread_id":"t","from_addr":"a@b.co","subject":"Tickets","received_at":"2026-09-17T00:00:00Z","snippet":"Preview","is_done":true,"subject_matches":["Tickets"],"snippet_matches":[]}"#.utf8)
        let hit = try decoder.decode(SearchHit.self, from: current)
        assert(hit.is_done == true && hit.subject_matches == ["Tickets"] && hit.snippet_matches == [])
        print("ok: search preview cleanup and wire compatibility")
    }
}
