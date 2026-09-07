// Copyright 2026 Mark Alan Boykin
// SPDX-License-Identifier: MPL-2.0
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import SwiftUI
import CoreNFC

struct ScanEvent: Codable, Identifiable {
    let id: UUID
    let session: UUID?
    let date: Date
    let elapsedMilliseconds: Int?
    let kind: String
    let detail: String
    let placement: String
    let caseNote: String
}

struct ScanReceipt: Codable {
    let schema: String
    let executionEnvironment: String?
    let operatingSystem: String
    let readingAvailable: Bool
    let scope: String
    let events: [ScanEvent]
}

// Both reader delegate queues are explicitly main. There is no background
// service, network client, controller credential, or device mutation here.
final class Probe: NSObject, ObservableObject, NFCNDEFReaderSessionDelegate,
                   NFCTagReaderSessionDelegate {
    @Published private(set) var events: [ScanEvent] = []
    @Published private(set) var busy = false
    @Published private(set) var status = "Ready for a foreground scan"
    @Published var placement = "Loose tag"
    @Published var caseNote = ""
    @Published private(set) var storageError: String?
    private var ndef: NFCNDEFReaderSession?
    private var tags: NFCTagReaderSession?
    private var sessionID: UUID?
    private var started: TimeInterval?
    private var sessionPlacement = ""
    private var sessionCase = ""
    private var deadline: DispatchWorkItem?

    var available: Bool { NFCReaderSession.readingAvailable }
    var executionEnvironment: String {
        #if targetEnvironment(simulator)
        return "simulator"
        #else
        return "physical-device"
        #endif
    }
    var receiptURL: URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("pocket-nfc-receipt.json")
    }

    override init() {
        super.init()
        if FileManager.default.fileExists(atPath: receiptURL.path) {
            do {
                let prior = try JSONDecoder().decode(ScanReceipt.self,
                    from: Data(contentsOf: receiptURL))
                guard prior.schema == "retinue.pocket-nfc-probe.v1" else {
                    throw CocoaError(.coderReadCorrupt)
                }
                events = Array(prior.events.suffix(128))
            } catch {
                // Preserve unreadable evidence for examination; the new log is separate.
                let backup = receiptURL.deletingLastPathComponent()
                    .appendingPathComponent("unreadable-\(UUID().uuidString).json")
                do { try FileManager.default.copyItem(at: receiptURL, to: backup) }
                catch { storageError = "Could not preserve the previous receipt." }
                status = "Previous receipt could not be read"
            }
        }
        if !available { status = "Scanning requires an NFC-capable physical phone" }
        record("app_open", available ? "NFC reader available" : "NFC reader unavailable")
    }

    private func record(_ kind: String, _ detail: String) {
        let elapsed = started.map { Int((ProcessInfo.processInfo.systemUptime - $0) * 1000) }
        let event = ScanEvent(id: UUID(), session: sessionID, date: Date(),
            elapsedMilliseconds: elapsed, kind: kind, detail: String(detail.prefix(256)),
            placement: sessionID == nil ? placement : sessionPlacement,
            caseNote: sessionID == nil ? String(caseNote.prefix(120)) : sessionCase)
        events.append(event)
        events = Array(events.suffix(128))
        persist()
        print("pocket_nfc \(kind): \(detail)")
    }

    private func persist() {
        let receipt = ScanReceipt(schema: "retinue.pocket-nfc-probe.v1",
            executionEnvironment: executionEnvironment,
            operatingSystem: UIDevice.current.systemVersion, readingAvailable: available,
            scope: "Foreground platform scan only; no authentication, mailbox, or RF proof",
            events: events)
        do {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            try encoder.encode(receipt).write(to: receiptURL, options: .atomic)
            storageError = nil
        } catch { storageError = "Receipt could not be saved. Free storage and retry." }
    }

    private func prepare(_ mode: String) -> Bool {
        guard !busy else { return false }
        guard available else {
            status = "NFC reading is unavailable on this device"
            record("unavailable", mode)
            return false
        }
        sessionID = UUID()
        sessionPlacement = placement
        sessionCase = String(caseNote.prefix(120))
        started = ProcessInfo.processInfo.systemUptime
        busy = true
        status = "Hold the test tag near the phone"
        record("scan_requested", mode)
        let work = DispatchWorkItem { [weak self] in self?.stop(reason: "probe_deadline") }
        deadline = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 30, execute: work)
        return true
    }

    func scanNDEF() {
        guard prepare("ndef") else { return }
        let session = NFCNDEFReaderSession(delegate: self, queue: .main,
            invalidateAfterFirstRead: true)
        ndef = session
        session.alertMessage = "Hold a test tag near the top of the phone."
        session.begin()
    }

    func scanISO15693() {
        guard prepare("iso15693") else { return }
        guard let session = NFCTagReaderSession(pollingOption: .iso15693,
            delegate: self, queue: .main) else {
            record("session_creation_failed", "iso15693")
            finish()
            return
        }
        tags = session
        session.alertMessage = "Hold the ISO15693 test tag near the phone."
        session.begin()
    }

    func stop(reason: String = "cancel_requested") {
        guard busy else { return }
        record(reason, "Reader invalidation requested")
        ndef?.invalidate()
        tags?.invalidate()
        // Keep the session until its invalidation callback, so a late callback
        // cannot be attributed to the next scan.
    }

    func becameInactive() {
        if busy { stop(reason: "app_inactive") }
    }

    private func finish() {
        deadline?.cancel()
        deadline = nil
        busy = false
        ndef = nil
        tags = nil
        sessionID = nil
        started = nil
    }

    private func invalidated(_ error: Error) {
        let e = error as NSError
        record("session_invalidated", "domain=\(e.domain) code=\(e.code)")
        status = "Scan ended. The receipt includes the result and session error code."
        finish()
    }

    func readerSessionDidBecomeActive(_ session: NFCNDEFReaderSession) {
        record("reader_active", "ndef")
    }
    func readerSession(_ session: NFCNDEFReaderSession, didInvalidateWithError error: Error) {
        guard ndef === session else { return }
        invalidated(error)
    }
    func readerSession(_ session: NFCNDEFReaderSession, didDetectNDEFs messages: [NFCNDEFMessage]) {
        guard ndef === session else { return }
        let records = messages.flatMap(\.records)
        let bytes = records.reduce(0) { $0 + $1.payload.count }
        // Do not retain tag contents or a stable tag UID in a shareable receipt.
        record("ndef_read", "messages=\(messages.count) records=\(records.count) payload_bytes=\(bytes)")
        status = "NDEF read succeeded"
    }
    func tagReaderSessionDidBecomeActive(_ session: NFCTagReaderSession) {
        record("reader_active", "iso15693")
    }
    func tagReaderSession(_ session: NFCTagReaderSession, didInvalidateWithError error: Error) {
        guard tags === session else { return }
        invalidated(error)
    }
    func tagReaderSession(_ session: NFCTagReaderSession, didDetect detected: [NFCTag]) {
        guard tags === session else { return }
        guard detected.count == 1, case let .iso15693(tag) = detected[0] else {
            record("tag_count_refused", "count=\(detected.count); present one ISO15693 tag")
            session.invalidate(errorMessage: "Present one ISO15693 tag and retry.")
            return
        }
        record("iso15693_detected", "manufacturer=\(tag.icManufacturerCode); mailbox not tested")
        status = "ISO15693 tag detected; mailbox not tested"
        session.alertMessage = "Tag detected. This scan does not test the mailbox."
        session.invalidate()
    }
}

struct ProbeView: View {
    @StateObject private var probe = Probe()
    @Environment(\.scenePhase) private var scenePhase
    var body: some View {
        NavigationStack {
            Form {
                Section("First phone proof") {
                    Text(probe.available ? "NFC reader available" : "NFC reader unavailable")
                    Text("Read-only scans. Authentication, mailbox exchange, and background launch are not tested here.")
                        .font(.footnote)
                    Text(probe.status).accessibilityIdentifier("probe.status")
                }
                Section("Test placement") {
                    Picker("Position", selection: $probe.placement) {
                        ForEach(["Loose tag", "Magnetic attachment position", "Other"], id: \.self) { Text($0) }
                    }
                    TextField("Case and orientation notes", text: $probe.caseNote)
                }.disabled(probe.busy)
                Section("Scan") {
                    Button("Read NDEF tag") { probe.scanNDEF() }.disabled(probe.busy || !probe.available)
                    Button("Detect ISO15693 tag") { probe.scanISO15693() }.disabled(probe.busy || !probe.available)
                    Button("Cancel scan") { probe.stop() }.disabled(!probe.busy)
                    Text("Each scan is limited to 30 seconds. Tag contents and tag identifiers are not saved.")
                        .font(.footnote)
                }
                Section("Local receipt") {
                    if let error = probe.storageError { Text(error).foregroundStyle(.red) }
                    ShareLink("Share scan receipt", item: probe.receiptURL)
                        .disabled(probe.storageError != nil)
                    Text("Keeps the latest 128 events on this phone. Sharing is your choice.").font(.footnote)
                    ForEach(probe.events.reversed()) { event in
                        VStack(alignment: .leading) {
                            Text(event.kind).font(.caption.bold())
                            Text(event.detail).font(.caption)
                            Text(event.date, style: .time).font(.caption2)
                        }
                    }
                }
            }.navigationTitle("Pocket NFC Probe")
        }.onChange(of: scenePhase) { _, phase in
            // The system reader sheet can make the app inactive; backgrounding
            // is the lifecycle boundary, not merely presenting that sheet.
            if phase == .background { probe.becameInactive() }
        }
    }
}

@main struct PocketNFCProbeApp: App {
    var body: some Scene { WindowGroup { ProbeView() } }
}
