// The shipment row as the daemon sends it, before and after order linking.
// An older daemon omits merchant / orders / legs entirely; a newer one sends
// them. Both must decode into the same `Shipment`, and the card's title rule
// must never fall back to "Package via <carrier>".

import Foundation

@main
@MainActor
struct ShipmentWireTests {
    static var failures = 0
    static var checks = 0

    static func main() {
        anOldRowDecodesWithoutTheOrderFields()
        aGroupedRowCarriesOrdersAndLegs()
        anUnknownLegCarrierDoesNotSinkTheRow()
        theTitleFallsBackToMerchantThenPackage()
        anOrderRefGetsExactlyOneHash()

        if failures > 0 {
            print("FAILED: \(failures) of \(checks) checks")
            exit(1)
        }
        print("ok: \(checks) checks passed")
    }

    static func expect(_ ok: Bool, _ what: String) {
        checks += 1
        if !ok {
            failures += 1
            print("  FAIL: \(what)")
        }
    }

    static let base = #""id":7,"account_id":1,"tracking_number":"1ZB8B2560323528551","carrier":"ups","status":"shipped","first_seen":"2026-09-20T10:00:00Z","last_update":"2026-09-22T10:00:00Z""#

    static func decode(_ json: String) -> Shipment? {
        try? JSONDecoder().decode(Shipment.self, from: Data(json.utf8))
    }

    static func anOldRowDecodesWithoutTheOrderFields() {
        let row = decode("{\(base),\"item_name\":\"\"}")
        expect(row != nil, "a row with no order fields decodes")
        expect(row?.merchant == nil && row?.orders == nil && row?.legs == nil, "absent fields are nil")
        expect(row?.displayTitle == "Package", "no name, no merchant: Package, never the carrier")
        expect(row?.orderLine == nil, "nothing to say: no order line")
    }

    static func aGroupedRowCarriesOrdersAndLegs() {
        let json = """
            {\(base),"item_name":"Austin Racing DB KILLER AUR10 x 1","merchant":"Bill's Exhausts",
             "orders":[{"merchant":"Bill's Exhausts","order_ref":"21470"}],
             "legs":[{"id":23,"carrier":"ups","tracking_number":"1ZW061R3DG21045729","status":"delivered",
                      "tracking_url":null,"last_update":"2026-09-18T10:00:00Z","delivered_at":"2026-09-18T09:00:00Z"}]}
            """
        let row = decode(json)
        expect(row != nil, "a grouped row decodes")
        expect(row?.legs?.first?.status == .delivered && row?.legs?.first?.id == 23, "the leg keeps its fields")
        expect(row?.displayTitle == "Austin Racing DB KILLER AUR10 x 1", "the item name is the title")
        expect(row?.orderLine == "Bill's Exhausts · #21470 · 2 packages", "merchant, ref, and box count")
    }

    static func anUnknownLegCarrierDoesNotSinkTheRow() {
        let json = """
            {\(base),"item_name":"x","legs":[{"id":1,"carrier":"ontrac","tracking_number":"T","status":"in_orbit",
             "last_update":"2026-09-18T10:00:00Z"}]}
            """
        let row = decode(json)
        expect(row?.legs?.first?.carrier == .unknown, "an unknown leg carrier falls back")
        expect(row?.legs?.first?.status == .ordered, "an unknown leg status falls back")
    }

    static func theTitleFallsBackToMerchantThenPackage() {
        let row = decode(#"{\#(base),"item_name":"","merchant":"  🎁 Bill's   Exhausts ","orders":[{"merchant":null,"order_ref":"21470"}]}"#)
        expect(row?.displayTitle == "Bill's Exhausts", "merchant stands in, emoji stripped, trimmed")
        expect(row?.orderLine == "#21470", "the merchant is not repeated under itself")
    }

    static func anOrderRefGetsExactlyOneHash() {
        let row = decode(##"{\##(base),"item_name":"x","orders":[{"merchant":null,"order_ref":"#1001"},{"merchant":null,"order_ref":"1002"}]}"##)
        expect(row?.orderLine == "#1001, #1002", "one hash each, joined with a comma")
    }
}
