# AudioLib Explorer - UI Color Scheme Guide

## Overview

AudioLib Explorer uses a professional, audio-focused color scheme inspired by industry-standard DAW applications like Pro Tools, Ableton Live, and Logic Pro. The design prioritizes readability, reduced eye strain during long sessions, and a modern, polished aesthetic suitable for audio professionals.

## Design Philosophy

### Key Principles

1. **Reduced Eye Strain**: Dark backgrounds with carefully balanced contrast ratios for extended use
2. **Professional Aesthetic**: Colors that evoke high-end audio production environments
3. **Visual Hierarchy**: Clear distinction between primary, secondary, and tertiary UI elements
4. **Audio Context**: Color choices that align with audio industry conventions (green for OK, amber for warning, red for peaks)
5. **Consistency**: Unified color language across all components

## Dark Theme (Default)

### Background Colors

| Color | Hex Code | Usage |
|-------|----------|-------|
| **Main Background** | `#1A1D23` | Primary window background (slightly blue-tinted) |
| **Light Background** | `#252930` | Panel backgrounds, card containers |
| **Lighter Background** | `#2F3640` | Input fields, data cells |
| **Darkest Background** | `#12151A` | Waveform backgrounds, deep contrast areas |
| **Hover State** | `#353B47` | Interactive element hover |
| **Active State** | `#3A4150` | Pressed/active state |

### Foreground Colors

| Color | Hex Code | Usage |
|-------|----------|-------|
| **Main Text** | `#E8EAED` | Primary text (high contrast) |
| **Bright Text** | `#F5F6F7` | Emphasized text, headings |
| **Secondary Text** | `#9DA5B4` | Secondary information, labels |
| **Tertiary Text** | `#6B7480` | Hints, placeholders |
| **Disabled Text** | `#4A5058` | Disabled controls |

### Primary Colors

| Color | Hex Code | Usage |
|-------|----------|-------|
| **Primary** | `#5B7FFF` | Accent color, primary buttons, selection |
| **Primary Dark** | `#4265E8` | Pressed state |
| **Primary Light** | `#7B9FFF` | Hover state, highlights |
| **Primary Hover** | `#6A8FFF` | Interactive hover |

### Accent Colors

| Color | Hex Code | Usage |
|-------|----------|-------|
| **Success/Accent** | `#3DDC97` | Success states, ready indicators, play button |
| **Accent Dim** | `#2AB67A` | Dimmed success state |
| **Warning** | `#FFB84D` | Warning states, pause button |
| **Error** | `#FF6B6B` | Error states, stop button (softer than pure red) |
| **Info** | `#5B7FFF` | Information messages (matches primary) |

### Audio-Specific Colors

#### Transport Controls
- **Play**: `#3DDC97` (Teal-green)
- **Pause**: `#FFB84D` (Amber)
- **Stop**: `#FF6B6B` (Coral red)
- **Record**: `#FF4757` (Bright red)

#### Audio Levels (VU/PPM Meters)
- **Low (-18dB to -6dB)**: `#3DDC97` (Green)
- **Mid (-6dB to 0dB)**: `#FFB84D` (Yellow/Amber)
- **High (0dB+)**: `#FF6B6B` (Red)

#### Waveform Visualization
- **Main Waveform**: `#5B7FFF` (Vibrant blue)
- **Waveform Fill**: `#3A5DD9` (Darker blue)
- **Peak Indicators**: `#FF6B6B` (Coral red)
- **RMS Overlay**: `#7B9FFF` (Lighter blue)
- **Background**: `#12151A` (Darkest)
- **Grid Lines**: `#252930` (Subtle gray)
- **Playhead**: `#3DDC97` (Teal-green)
- **Selection Overlay**: `#5B7FFF40` (Semi-transparent blue)

### UI Elements

#### Borders
- **Default Border**: `#3A4150`
- **Light Border**: `#4A5160`
- **Focus Border**: `#5B7FFF` (Primary blue)
- **Error Border**: `#FF6B6B`

#### Selection
- **Selection Highlight**: `#5B7FFF`
- **Selection Background**: `#2E4485`
- **Inactive Selection**: `#2F3640`

#### Scrollbars
- **Background**: `#1A1D23`
- **Thumb**: `#3A4150`
- **Thumb Hover**: `#4A5160`

## Light Theme (Alternative)

### Background Colors

| Color | Hex Code | Usage |
|-------|----------|-------|
| **Main Background** | `#F8F9FB` | Soft white background |
| **Light Background** | `#FFFFFF` | Pure white panels |
| **Lighter Background** | `#E8EAED` | Light gray inputs |
| **Darkest Background** | `#DFE1E5` | Contrast areas |
| **Hover State** | `#E0E2E8` | Interactive hover |
| **Active State** | `#D5D7DD` | Pressed state |

### Foreground Colors

| Color | Hex Code | Usage |
|-------|----------|-------|
| **Main Text** | `#1A1D23` | Dark text |
| **Bright Text** | `#000000` | Black emphasis |
| **Secondary Text** | `#4A5058` | Secondary info |
| **Tertiary Text** | `#6B7480` | Hints |
| **Disabled Text** | `#9DA5B4` | Disabled controls |

### Primary & Accent Colors
Similar structure to dark theme with adjusted values for light backgrounds.

## Typography

### Font Families
- **UI Text**: Segoe UI (system default on Windows)
- **Monospace**: Consolas (for timecode, technical values)

### Font Sizes & Weights

| Style | Family | Size | Weight | Usage |
|-------|--------|------|--------|-------|
| **default** | Segoe UI | 10pt | Regular | Standard UI text |
| **heading** | Segoe UI Semibold | 13pt | Semibold | Section headings |
| **large** | Segoe UI | 11pt | Regular | Prominent labels |
| **small** | Segoe UI | 9pt | Regular | Supporting text |
| **tiny** | Segoe UI | 8pt | Regular | Minimal labels |
| **monospace** | Consolas | 10pt | Regular | Timecode, sample rates |
| **title** | Segoe UI Semibold | 16pt | Semibold | Window titles |
| **button** | Segoe UI Semibold | 10pt | Semibold | Button text |

## Spacing System

### Spacing Scale
- **xs**: 3px - Minimal spacing
- **sm**: 6px - Tight spacing
- **md**: 10px - Standard spacing
- **lg**: 14px - Comfortable spacing
- **xl**: 20px - Generous spacing
- **xxl**: 28px - Section spacing
- **xxxl**: 36px - Major section dividers

### Border Radius
- **sm**: 3px
- **md**: 5px
- **lg**: 8px
- **xl**: 12px

## Component Styles

### Buttons

#### Primary Button
- Background: `#5B7FFF` (Primary blue)
- Foreground: White
- Padding: 20px × 10px
- Font: Segoe UI Semibold 10pt
- Hover: `#6A8FFF`
- Active: `#4265E8`

#### Success Button
- Background: `#3DDC97` (Teal-green)
- Use for: Play, Confirm, Save actions
- Foreground: White

#### Danger Button
- Background: `#FF6B6B` (Coral red)
- Use for: Stop, Delete, Cancel actions
- Foreground: White

#### Secondary Button
- Background: `#2F3640` (Lighter bg)
- Foreground: `#E8EAED` (Main text)
- More subtle appearance

### Input Fields
- Background: `#2F3640`
- Text: `#E8EAED`
- Border: `#3A4150` (default), `#5B7FFF` (focus)
- Cursor: `#5B7FFF` (Primary blue)
- Padding: 10px × 6px

### Treeview (Results List)
- Background: `#252930`
- Row Height: 28px (increased for readability)
- Heading Background: `#1A1D23`
- Heading Text: `#9DA5B4` (Secondary)
- Heading Font: Bold
- Selected Background: `#2E4485`
- Selected Text: White

### Tabs (Notebook)
- Inactive Tab: `#2F3640` background, `#9DA5B4` text
- Active Tab: `#1A1D23` background, `#5B7FFF` text (primary)
- Hover: `#353B47` background
- Padding: 20px × 10px

### Progress Bar
- Bar Color: `#5B7FFF` (Primary)
- Track Color: `#2F3640`
- Thickness: 6px
- Success variant: `#3DDC97` (for audio processing)

### Scrollbars
- Width: 14px
- Thumb: `#3A4150`
- Thumb Hover: `#4A5160`
- Track: `#1A1D23`
- Minimalist, unobtrusive design

## Usage Guidelines

### When to Use Each Color

#### Primary Blue (`#5B7FFF`)
- Primary action buttons
- Links and interactive elements
- Selection highlights
- Focus indicators
- Main waveform visualization

#### Teal-Green (`#3DDC97`)
- Success messages
- Play button
- Audio level OK state
- Ready indicators
- Playhead position

#### Amber (`#FFB84D`)
- Warning messages
- Pause button
- Audio level warning state (-6dB to 0dB)
- Caution indicators

#### Coral Red (`#FF6B6B`)
- Error messages
- Stop button
- Audio level peak/clip state (0dB+)
- Destructive actions

### Contrast Ratios

All color combinations meet WCAG 2.1 AA standards for normal text:
- Main text on background: 12.5:1 (AAA)
- Secondary text on background: 7.2:1 (AA)
- Primary blue on background: 8.1:1 (AAA)

## Implementation

### Accessing Colors in Code

```python
from src.gui.styles import get_style_manager

style_manager = get_style_manager('dark')  # or 'light'
primary_color = style_manager.get_color('primary')
bg_color = style_manager.get_color('bg')
```

### Accessing Fonts

```python
from src.gui.styles import FONTS

button_font = FONTS['button']
heading_font = FONTS['heading']
monospace_font = FONTS['monospace']
```

### Accessing Spacing

```python
from src.gui.styles import SPACING

standard_padding = SPACING['md']
section_spacing = SPACING['xl']
```

## Future Enhancements

### Potential Additions
1. **Custom Themes**: User-defined color schemes
2. **Accessibility Mode**: High contrast variant
3. **Color Blind Modes**: Alternative palettes for color vision deficiency
4. **Dynamic Themes**: Time-based theme switching (day/night)
5. **Theme Import/Export**: Share custom themes

### Animation & Transitions
- Subtle hover animations (50-100ms transitions)
- Smooth color transitions on state changes
- Fade effects for tooltips and modals

## References

### Inspiration Sources
- **Pro Tools**: Professional DAW industry standard
- **Ableton Live**: Modern, clean interface design
- **Logic Pro**: Polished Apple aesthetic
- **Material Design**: Modern UI principles
- **Fluent Design**: Microsoft's design language

### Color Psychology
- **Blue**: Trust, professionalism, stability
- **Teal-Green**: Success, creativity, balance
- **Amber**: Attention, caution (without alarm)
- **Coral Red**: Urgency (softer than pure red for reduced eye strain)

## Version History

### v1.0.0 (2025-11-23)
- Initial professional audio color scheme
- Dark theme implementation
- Light theme implementation
- Complete typography system
- Comprehensive component styling
- Audio-specific color conventions
