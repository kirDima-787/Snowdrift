import Foundation
import IOKit.ps

/// How much the snow may spend on drawing.
enum PowerBudget {
    case full
    /// On battery: no frame rates above 60, which are hard to tell apart in snow anyway.
    case battery
    /// Low Power Mode, or a Mac running hot: a little less snow, drawn a little less often.
    case saver
}

final class PowerMonitor {
    private(set) var budget = PowerBudget.full
    var changed: () -> Void = {}
    private var source: CFRunLoopSource?
    private var observers: [NSObjectProtocol] = []

    init() {
        let context = Unmanaged.passUnretained(self).toOpaque()
        source = IOPSNotificationCreateRunLoopSource({ context in
            guard let context else { return }
            Unmanaged<PowerMonitor>.fromOpaque(context).takeUnretainedValue().update()
        }, context)?.takeRetainedValue()
        if let source { CFRunLoopAddSource(CFRunLoopGetMain(), source, .defaultMode) }
        for name in [Notification.Name.NSProcessInfoPowerStateDidChange, ProcessInfo.thermalStateDidChangeNotification] {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                self?.update()
            })
        }
        update()
    }

    deinit {
        if let source { CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .defaultMode) }
    }

    private func update() {
        let info = ProcessInfo.processInfo
        let next: PowerBudget
        if info.isLowPowerModeEnabled || info.thermalState == .serious || info.thermalState == .critical {
            next = .saver
        } else if onBattery {
            next = .battery
        } else {
            next = .full
        }
        guard next != budget else { return }
        budget = next
        changed()
    }

    private var onBattery: Bool {
        guard let info = IOPSCopyPowerSourcesInfo()?.takeRetainedValue(),
              let type = IOPSGetProvidingPowerSourceType(info)?.takeUnretainedValue()
        else { return false }
        return (type as String) == kIOPMBatteryPowerKey
    }
}
