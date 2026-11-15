/* Slideshow shown while Nimbus installs. */
import QtQuick 2.15
import calamares.slideshow 1.0

Presentation {
    id: presentation

    function nextSlide() { presentation.goToNextSlide(); }

    Timer {
        interval: 7000
        running: presentation.activatedInCalamares
        repeat: true
        onTriggered: nextSlide()
    }

    Slide {
        Rectangle { anchors.fill: parent; color: "#141826" }
        Image {
            id: logo1
            source: "logo.png"
            width: 128; height: 128
            fillMode: Image.PreserveAspectFit
            anchors.centerIn: parent
            anchors.verticalCenterOffset: -60
        }
        Text {
            anchors.top: logo1.bottom; anchors.topMargin: 24
            anchors.horizontalCenter: parent.horizontalCenter
            color: "#e6e9f2"; font.pixelSize: 26; font.bold: true
            text: "Welcome to Nimbus"
        }
    }

    Slide {
        Rectangle { anchors.fill: parent; color: "#141826" }
        Column {
            anchors.centerIn: parent; spacing: 14; width: parent.width * 0.7
            Text { color: "#5aa9ff"; font.pixelSize: 24; font.bold: true; text: "Tap Super to find anything" }
            Text {
                width: parent.width; wrapMode: Text.WordWrap; color: "#e6e9f2"; font.pixelSize: 16
                text: "In Nimbus Desktop, tap the Super (Windows/⌘) key to open the launcher and start typing. Super+Enter opens a terminal, Super+E opens your files."
            }
        }
    }

    Slide {
        Rectangle { anchors.fill: parent; color: "#141826" }
        Column {
            anchors.centerIn: parent; spacing: 14; width: parent.width * 0.7
            Text { color: "#5aa9ff"; font.pixelSize: 24; font.bold: true; text: "Arch Linux underneath" }
            Text {
                width: parent.width; wrapMode: Text.WordWrap; color: "#e6e9f2"; font.pixelSize: 16
                text: "Nimbus is rolling-release and uses pacman. Keep it fresh with: sudo pacman -Syu"
            }
        }
    }

    function onActivate() { presentation.currentSlide = 0; }
    function onLeave() { }
}
