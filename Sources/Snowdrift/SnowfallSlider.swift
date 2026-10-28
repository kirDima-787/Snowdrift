import AppKit

/// The Snowfall menu control: a slider with a stop per option and the chosen option's name under it.
final class SnowfallSlider: NSView {
    var changed: ((Snowfall) -> Void)?

    var snowfall = Snowfall.steady {
        didSet { show() }
    }

    private let slider = NSSlider()
    private let caption = NSTextField(labelWithString: "")

    init() {
        super.init(frame: NSRect(x: 0, y: 0, width: 200, height: 50))
        autoresizingMask = .width

        let stops = Snowfall.allCases.count
        slider.minValue = 0
        slider.maxValue = Double(stops - 1)
        slider.numberOfTickMarks = stops
        slider.allowsTickMarkValuesOnly = true
        slider.target = self
        slider.action = #selector(slide)

        caption.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        caption.textColor = .secondaryLabelColor
        caption.alignment = .center

        for view in [slider, caption] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        // The track's ends line up with the item titles and the separators of a macOS 26 menu.
        NSLayoutConstraint.activate([
            slider.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 29),
            slider.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -15),
            slider.topAnchor.constraint(equalTo: topAnchor, constant: 4),
            caption.centerXAnchor.constraint(equalTo: slider.centerXAnchor),
            caption.topAnchor.constraint(equalTo: slider.bottomAnchor, constant: 2),
            caption.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -2),
        ])
        setFrameSize(NSSize(width: frame.width, height: fittingSize.height))
        show()
    }

    required init?(coder: NSCoder) { nil }

    @objc private func slide() {
        guard let next = Snowfall(rawValue: slider.integerValue), next != snowfall else { return }
        snowfall = next
        changed?(next)
    }

    private func show() {
        slider.integerValue = snowfall.rawValue
        slider.setAccessibilityValueDescription(snowfall.title)
        caption.stringValue = snowfall.title
    }
}
