# UI Redesign Proposal: Directory Browser Integration

## Overview

This document describes the UI redesign for AudioLib Explorer, replacing the right Favorites/Spotting panel with a **Directory Browser** tree and reorganizing the layout for optimal cross-indexed navigation.

---

## 1. Layout Redesign

### 1.1 Current Layout

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Menu Bar                                    │
├───────────────┬────────────────────────────────┬────────────────────┤
│               │                                │                    │
│  Left Panel   │      Center Panel              │   Right Panel      │
│  (Filters)    │      (Results + Waveform)      │   (Favorites)      │
│               │                                │   TO BE REPLACED   │
│  - Search     │  ┌────────────────────────┐   │                    │
│  - Type       │  │   Results Table        │   │  - Favorites Tree  │
│  - Rate       │  │   (Sortable columns)   │   │  - Spotting        │
│  - Depth      │  └────────────────────────┘   │  - Recent          │
│  - Duration   │  ┌────────────────────────┐   │                    │
│  - Checkboxes │  │   Waveform Display     │   │                    │
│               │  └────────────────────────┘   │                    │
│               │                                │                    │
├───────────────┴────────────────────────────────┴────────────────────┤
│                     Bottom Player Bar                               │
└─────────────────────────────────────────────────────────────────────┘
```

**Current proportions**: 20% | 55% | 25%

### 1.2 New Layout

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Menu Bar                                    │
├───────────────┬────────────────────────────────┬────────────────────┤
│               │                                │                    │
│  Left Panel   │      Center Panel              │   Right Panel      │
│  (DIRECTORY)  │      (Results + Waveform)      │   (FILTERS)        │
│               │                                │                    │
│  ┌─────────┐  │  ┌────────────────────────┐   │  ┌──────────────┐  │
│  │ Toolbar │  │  │ Breadcrumb: Root > SFX │   │  │ Search Box   │  │
│  └─────────┘  │  └────────────────────────┘   │  └──────────────┘  │
│  ┌─────────┐  │  ┌────────────────────────┐   │  ┌──────────────┐  │
│  │ Folder  │  │  │   Results Table        │   │  │ Type Filter  │  │
│  │ Tree    │  │  │   (with folder column) │   │  │ Rate Filter  │  │
│  │         │  │  │                        │   │  │ Depth Filter │  │
│  │ 📁 Root │  │  └────────────────────────┘   │  │ Duration     │  │
│  │  └📁SFX │  │  ┌────────────────────────┐   │  │ Checkboxes   │  │
│  │   └📁... │  │  │   Waveform Display     │   │  └──────────────┘  │
│  └─────────┘  │  └────────────────────────┘   │  ┌──────────────┐  │
│  ┌─────────┐  │                                │  │ Apply/Reset  │  │
│  │ Stats   │  │                                │  └──────────────┘  │
│  └─────────┘  │                                │                    │
├───────────────┴────────────────────────────────┴────────────────────┤
│                     Bottom Player Bar                               │
└─────────────────────────────────────────────────────────────────────┘
```

**New proportions**: 22% | 53% | 25%

---

## 2. New Directory Browser Panel

### 2.1 Component Structure

```python
# src/gui_qt/components/directory_browser.py

from PyQt6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QTreeWidgetItem
)
from PyQt6.QtCore import pyqtSignal, Qt
from qfluentwidgets import (
    CardWidget, TreeWidget, PushButton, TransparentToolButton,
    SubtitleLabel, CaptionLabel, ToolTipFilter
)
from qfluentwidgets import FluentIcon as FIF


class DirectoryBrowserPanel(CardWidget):
    """
    Left panel: Directory tree browser with cross-index filtering.

    Signals:
        folderSelected(int, str): Emitted when folder clicked (id, path)
        addRootRequested(): Emitted when Add Root button clicked
        refreshRequested(): Emitted when Refresh button clicked
    """

    folderSelected = pyqtSignal(int, str)
    addRootRequested = pyqtSignal()
    refreshRequested = pyqtSignal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("directoryBrowserPanel")
        self._setup_ui()
        self._connect_signals()

    def _setup_ui(self):
        """Setup UI components"""
        layout = QVBoxLayout(self)
        layout.setContentsMargins(16, 16, 16, 16)
        layout.setSpacing(12)

        # Header
        header_layout = QHBoxLayout()
        self.title_label = SubtitleLabel("Library Browser")
        header_layout.addWidget(self.title_label)
        header_layout.addStretch()

        # Collapse button
        self.collapse_btn = TransparentToolButton(FIF.CHEVRON_LEFT)
        self.collapse_btn.setToolTip("Collapse panel (Ctrl+1)")
        self.collapse_btn.installEventFilter(ToolTipFilter(self.collapse_btn))
        header_layout.addWidget(self.collapse_btn)
        layout.addLayout(header_layout)

        # Toolbar
        toolbar_layout = QHBoxLayout()
        toolbar_layout.setSpacing(8)

        self.add_root_btn = PushButton(FIF.FOLDER_ADD, "Add Root")
        self.add_root_btn.setToolTip("Add a root folder to the library")
        toolbar_layout.addWidget(self.add_root_btn)

        toolbar_layout.addStretch()

        self.refresh_btn = TransparentToolButton(FIF.SYNC)
        self.refresh_btn.setToolTip("Refresh folder tree")
        toolbar_layout.addWidget(self.refresh_btn)

        self.collapse_all_btn = TransparentToolButton(FIF.CHEVRON_UP)
        self.collapse_all_btn.setToolTip("Collapse all folders")
        toolbar_layout.addWidget(self.collapse_all_btn)

        self.expand_all_btn = TransparentToolButton(FIF.CHEVRON_DOWN)
        self.expand_all_btn.setToolTip("Expand all folders")
        toolbar_layout.addWidget(self.expand_all_btn)

        layout.addLayout(toolbar_layout)

        # Folder Tree
        self.tree = TreeWidget()
        self.tree.setHeaderHidden(True)
        self.tree.setIndentation(20)
        self.tree.setAnimated(True)
        self.tree.setExpandsOnDoubleClick(True)
        self.tree.setAlternatingRowColors(True)
        layout.addWidget(self.tree, 1)  # Stretch factor 1

        # Statistics Footer
        self.stats_label = CaptionLabel("0 folders, 0 files")
        self.stats_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        layout.addWidget(self.stats_label)

    def _connect_signals(self):
        """Connect internal signals"""
        self.tree.itemClicked.connect(self._on_item_clicked)
        self.tree.itemExpanded.connect(self._on_item_expanded)
        self.tree.itemCollapsed.connect(self._on_item_collapsed)
        self.add_root_btn.clicked.connect(self.addRootRequested.emit)
        self.refresh_btn.clicked.connect(self.refreshRequested.emit)
        self.collapse_all_btn.clicked.connect(self.tree.collapseAll)
        self.expand_all_btn.clicked.connect(self.tree.expandAll)

    def _on_item_clicked(self, item: QTreeWidgetItem, column: int):
        """Handle folder selection"""
        folder_id = item.data(0, Qt.ItemDataRole.UserRole)
        folder_path = item.data(0, Qt.ItemDataRole.UserRole + 1)
        if folder_id is not None:
            self.folderSelected.emit(folder_id, folder_path)

    def _on_item_expanded(self, item: QTreeWidgetItem):
        """Handle folder expansion - lazy load children"""
        # Check if children need to be loaded
        if item.childCount() == 1:
            placeholder = item.child(0)
            if placeholder.data(0, Qt.ItemDataRole.UserRole) == "placeholder":
                folder_id = item.data(0, Qt.ItemDataRole.UserRole)
                self._request_children(folder_id, item)

    def _on_item_collapsed(self, item: QTreeWidgetItem):
        """Handle folder collapse"""
        pass  # Could clear children to save memory for huge trees

    def _request_children(self, folder_id: int, parent_item: QTreeWidgetItem):
        """Request child folders from service bridge"""
        # This will be connected to ServiceBridge.load_folder_tree
        # Children are loaded asynchronously
        pass

    # --- Public API ---

    def set_root_folders(self, folders: list):
        """
        Set root-level folders.

        Args:
            folders: List of dicts with keys:
                - id: folder ID
                - name: display name
                - path: full path
                - file_count: total file count
                - has_children: bool
        """
        self.tree.clear()

        for folder in folders:
            item = self._create_folder_item(folder)
            self.tree.addTopLevelItem(item)

        self._update_stats()

    def set_child_folders(self, parent_id: int, folders: list):
        """
        Set children for a parent folder.

        Args:
            parent_id: Parent folder ID
            folders: List of child folder dicts
        """
        parent_item = self._find_item_by_id(parent_id)
        if not parent_item:
            return

        # Remove placeholder
        parent_item.takeChildren()

        # Add children
        for folder in folders:
            item = self._create_folder_item(folder)
            parent_item.addChild(item)

    def update_folder_count(self, folder_id: int, count: int):
        """Update displayed file count for a folder"""
        item = self._find_item_by_id(folder_id)
        if item:
            name = item.data(0, Qt.ItemDataRole.UserRole + 2)  # Original name
            item.setText(0, f"{name} ({count})")

    def clear_selection(self):
        """Clear folder selection"""
        self.tree.clearSelection()

    def select_folder(self, folder_id: int):
        """Select a folder by ID"""
        item = self._find_item_by_id(folder_id)
        if item:
            self.tree.setCurrentItem(item)
            self.tree.scrollToItem(item)

    def get_selected_folder_id(self) -> int | None:
        """Get currently selected folder ID"""
        item = self.tree.currentItem()
        if item:
            return item.data(0, Qt.ItemDataRole.UserRole)
        return None

    # --- Private Helpers ---

    def _create_folder_item(self, folder: dict) -> QTreeWidgetItem:
        """Create a tree widget item for a folder"""
        name = folder['name']
        count = folder.get('file_count', 0)
        display_text = f"{name} ({count})" if count > 0 else name

        item = QTreeWidgetItem([display_text])

        # Store data
        item.setData(0, Qt.ItemDataRole.UserRole, folder['id'])
        item.setData(0, Qt.ItemDataRole.UserRole + 1, folder['path'])
        item.setData(0, Qt.ItemDataRole.UserRole + 2, name)  # Original name

        # Set icon
        item.setIcon(0, FIF.FOLDER.icon())

        # Add placeholder child if has children (for lazy loading)
        if folder.get('has_children', False):
            placeholder = QTreeWidgetItem(["Loading..."])
            placeholder.setData(0, Qt.ItemDataRole.UserRole, "placeholder")
            item.addChild(placeholder)

        return item

    def _find_item_by_id(self, folder_id: int) -> QTreeWidgetItem | None:
        """Find tree item by folder ID"""
        def search(item: QTreeWidgetItem) -> QTreeWidgetItem | None:
            if item.data(0, Qt.ItemDataRole.UserRole) == folder_id:
                return item
            for i in range(item.childCount()):
                found = search(item.child(i))
                if found:
                    return found
            return None

        for i in range(self.tree.topLevelItemCount()):
            found = search(self.tree.topLevelItem(i))
            if found:
                return found
        return None

    def _update_stats(self):
        """Update statistics footer"""
        folder_count = self._count_all_items()
        # File count would require additional data
        self.stats_label.setText(f"{folder_count} folders")

    def _count_all_items(self) -> int:
        """Count all items in tree"""
        count = 0

        def count_children(item: QTreeWidgetItem):
            nonlocal count
            count += 1
            for i in range(item.childCount()):
                child = item.child(i)
                if child.data(0, Qt.ItemDataRole.UserRole) != "placeholder":
                    count_children(child)

        for i in range(self.tree.topLevelItemCount()):
            count_children(self.tree.topLevelItem(i))

        return count
```

### 2.2 Tree Item Visual Design

```
┌────────────────────────────────────────────┐
│ 📁 SFX Library (1,234)                     │  ← Root folder with count
│   ├─ 📁 Explosions (456)                   │  ← Expanded subfolder
│   │   ├─ 📁 Large (123)                    │
│   │   ├─ 📁 Medium (200)                   │
│   │   └─ 📁 Small (133)                    │
│   ├─ 📁 Foley (389)                        │  ← Collapsed (▶ icon)
│   └─ 📁 Ambience (389)                     │
│ 📁 Music Library (5,678)                   │  ← Another root
│   └─ 📁 Stems (2,345)                      │
└────────────────────────────────────────────┘
```

**Visual Elements**:
- Folder icon: `FIF.FOLDER` (yellow folder)
- Expand/collapse: Built-in tree arrows
- File count: Gray text in parentheses
- Selected: Fluent accent color highlight
- Alternating rows: Subtle background variation

---

## 3. Modified Filter Panel (Right Side)

### 3.1 Relocated Components

```python
# src/gui_qt/components/filter_panel.py (modified from LeftFilterPanel)

class FilterPanel(CardWidget):
    """
    Right panel: Search and filter controls.

    Moved from left to right to accommodate directory browser.
    """

    filtersChanged = pyqtSignal(str, dict)  # (query, filters)
    filtersCleared = pyqtSignal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("filterPanel")
        self._setup_ui()

    def _setup_ui(self):
        layout = QVBoxLayout(self)
        layout.setContentsMargins(16, 16, 16, 16)
        layout.setSpacing(12)

        # Header
        header_layout = QHBoxLayout()
        self.title_label = SubtitleLabel("Filters")
        header_layout.addWidget(self.title_label)
        header_layout.addStretch()

        self.collapse_btn = TransparentToolButton(FIF.CHEVRON_RIGHT)
        self.collapse_btn.setToolTip("Collapse panel (Ctrl+2)")
        header_layout.addWidget(self.collapse_btn)
        layout.addLayout(header_layout)

        # Search Box
        self.search_box = SearchLineEdit()
        self.search_box.setPlaceholderText("Search files, tags, metadata...")
        layout.addWidget(self.search_box)

        # Current Folder Display (NEW)
        self.folder_section = QWidget()
        folder_layout = QHBoxLayout(self.folder_section)
        folder_layout.setContentsMargins(0, 0, 0, 0)

        self.folder_label = CaptionLabel("Folder:")
        folder_layout.addWidget(self.folder_label)

        self.folder_path_label = BodyLabel("All folders")
        self.folder_path_label.setWordWrap(True)
        folder_layout.addWidget(self.folder_path_label, 1)

        self.clear_folder_btn = TransparentToolButton(FIF.CLOSE)
        self.clear_folder_btn.setToolTip("Clear folder filter")
        self.clear_folder_btn.setVisible(False)
        folder_layout.addWidget(self.clear_folder_btn)

        layout.addWidget(self.folder_section)

        # Divider
        layout.addWidget(self._create_divider())

        # Filter dropdowns (same as before)
        self.file_type_combo = self._create_filter_combo(
            "File Type", ["All", "WAV", "AIFF", "MP3", "FLAC"]
        )
        layout.addWidget(self.file_type_combo)

        self.sample_rate_combo = self._create_filter_combo(
            "Sample Rate", ["All", "44.1 kHz", "48 kHz", "96 kHz", "192 kHz"]
        )
        layout.addWidget(self.sample_rate_combo)

        self.bit_depth_combo = self._create_filter_combo(
            "Bit Depth", ["All", "16-bit", "24-bit", "32-bit float"]
        )
        layout.addWidget(self.bit_depth_combo)

        self.channels_combo = self._create_filter_combo(
            "Channels", ["All", "Mono", "Stereo", "Multi-channel"]
        )
        layout.addWidget(self.channels_combo)

        # Duration range
        duration_layout = QHBoxLayout()
        self.duration_min = LineEdit()
        self.duration_min.setPlaceholderText("Min (s)")
        duration_layout.addWidget(self.duration_min)

        duration_layout.addWidget(CaptionLabel("to"))

        self.duration_max = LineEdit()
        self.duration_max.setPlaceholderText("Max (s)")
        duration_layout.addWidget(self.duration_max)
        layout.addLayout(duration_layout)

        # Checkboxes
        self.tagged_checkbox = CheckBox("Tagged files only")
        layout.addWidget(self.tagged_checkbox)

        self.favorites_checkbox = CheckBox("Favorites only")
        layout.addWidget(self.favorites_checkbox)

        self.referenced_checkbox = CheckBox("Referenced in projects")
        layout.addWidget(self.referenced_checkbox)

        layout.addStretch()

        # Action buttons
        button_layout = QHBoxLayout()
        self.apply_btn = PrimaryPushButton("Apply Filters")
        button_layout.addWidget(self.apply_btn)

        self.reset_btn = PushButton("Reset All")
        button_layout.addWidget(self.reset_btn)
        layout.addLayout(button_layout)

    def set_selected_folder(self, folder_path: str | None):
        """Update the folder display"""
        if folder_path:
            # Truncate long paths
            if len(folder_path) > 40:
                folder_path = "..." + folder_path[-37:]
            self.folder_path_label.setText(folder_path)
            self.clear_folder_btn.setVisible(True)
        else:
            self.folder_path_label.setText("All folders")
            self.clear_folder_btn.setVisible(False)
```

---

## 4. Center Panel Modifications

### 4.1 Add Breadcrumb Navigation

```python
# src/gui_qt/components/breadcrumb.py

from PyQt6.QtWidgets import QWidget, QHBoxLayout
from PyQt6.QtCore import pyqtSignal
from qfluentwidgets import (
    PushButton, TransparentToolButton, CaptionLabel, HyperlinkButton
)
from qfluentwidgets import FluentIcon as FIF


class BreadcrumbWidget(QWidget):
    """
    Breadcrumb navigation showing current folder path.

    Example: Root > SFX > Explosions > Large
    """

    folderClicked = pyqtSignal(int, str)  # (folder_id, folder_path)
    homeClicked = pyqtSignal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self._setup_ui()
        self._crumbs = []

    def _setup_ui(self):
        self.layout = QHBoxLayout(self)
        self.layout.setContentsMargins(0, 0, 0, 0)
        self.layout.setSpacing(4)

        # Home button
        self.home_btn = TransparentToolButton(FIF.HOME)
        self.home_btn.setToolTip("Show all folders")
        self.home_btn.clicked.connect(self.homeClicked.emit)
        self.layout.addWidget(self.home_btn)

        self.layout.addStretch()

    def set_path(self, folders: list):
        """
        Set breadcrumb path.

        Args:
            folders: List of dicts with 'id', 'name', 'path' from root to current
        """
        # Clear existing crumbs
        for crumb in self._crumbs:
            crumb.deleteLater()
        self._crumbs.clear()

        # Remove stretch
        self.layout.removeItem(self.layout.itemAt(self.layout.count() - 1))

        # Add crumbs
        for i, folder in enumerate(folders):
            if i > 0:
                separator = CaptionLabel(">")
                self.layout.addWidget(separator)
                self._crumbs.append(separator)

            btn = HyperlinkButton(folder['name'])
            btn.setProperty('folder_id', folder['id'])
            btn.setProperty('folder_path', folder['path'])
            btn.clicked.connect(self._on_crumb_clicked)
            self.layout.addWidget(btn)
            self._crumbs.append(btn)

        # Re-add stretch
        self.layout.addStretch()

    def _on_crumb_clicked(self):
        """Handle breadcrumb click"""
        btn = self.sender()
        folder_id = btn.property('folder_id')
        folder_path = btn.property('folder_path')
        self.folderClicked.emit(folder_id, folder_path)

    def clear(self):
        """Clear breadcrumb"""
        for crumb in self._crumbs:
            crumb.deleteLater()
        self._crumbs.clear()
```

### 4.2 Add Folder Column to Results Table

```python
# Modify CenterResultsPanel to include folder column

class CenterResultsPanel(QWidget):
    def _setup_results_table(self):
        self.results_table = TableWidget()
        self.results_table.setColumnCount(9)  # Was 8

        headers = [
            "Filename",
            "Duration",
            "Sample Rate",
            "Bit Depth",
            "Channels",
            "Size",
            "Tags",
            "Folder",      # NEW column
            "Referenced"
        ]
        self.results_table.setHorizontalHeaderLabels(headers)

        # Column widths
        self.results_table.setColumnWidth(0, 250)  # Filename
        self.results_table.setColumnWidth(1, 80)   # Duration
        self.results_table.setColumnWidth(2, 100)  # Sample Rate
        self.results_table.setColumnWidth(3, 80)   # Bit Depth
        self.results_table.setColumnWidth(4, 80)   # Channels
        self.results_table.setColumnWidth(5, 80)   # Size
        self.results_table.setColumnWidth(6, 150)  # Tags
        self.results_table.setColumnWidth(7, 200)  # Folder (NEW)
        self.results_table.setColumnWidth(8, 100)  # Referenced
```

---

## 5. Main Window Integration

### 5.1 Updated Panel Layout

```python
# src/gui_qt/main_window_soundminer.py

class AudioLibMainWindow(QMainWindow):
    def _setup_ui(self):
        # ... existing setup ...

        # Main content area with splitter
        main_splitter = QSplitter(Qt.Orientation.Horizontal)

        # LEFT: Directory Browser (NEW - replaces nothing, adds new panel)
        self.directory_panel = DirectoryBrowserPanel()
        main_splitter.addWidget(self.directory_panel)

        # CENTER: Results + Waveform
        self.center_panel = CenterResultsPanel()
        main_splitter.addWidget(self.center_panel)

        # RIGHT: Filters (MOVED from left)
        self.filter_panel = FilterPanel()
        main_splitter.addWidget(self.filter_panel)

        # Set proportions: 22% | 53% | 25%
        main_splitter.setSizes([350, 850, 400])

        # Collapsibility
        main_splitter.setCollapsible(0, True)   # Directory can collapse
        main_splitter.setCollapsible(1, False)  # Results cannot collapse
        main_splitter.setCollapsible(2, True)   # Filters can collapse

        # ... rest of setup ...

    def _connect_signals(self):
        # Directory browser signals
        self.directory_panel.folderSelected.connect(self._on_folder_selected)
        self.directory_panel.addRootRequested.connect(self._on_add_root_requested)
        self.directory_panel.refreshRequested.connect(self._on_refresh_requested)

        # Filter panel signals
        self.filter_panel.filtersChanged.connect(self._on_filters_changed)
        self.filter_panel.filtersCleared.connect(self._on_filters_cleared)
        self.filter_panel.clear_folder_btn.clicked.connect(self._on_clear_folder)

        # Breadcrumb signals
        self.center_panel.breadcrumb.folderClicked.connect(self._on_folder_selected)
        self.center_panel.breadcrumb.homeClicked.connect(self._on_clear_folder)

    def _on_folder_selected(self, folder_id: int, folder_path: str):
        """Handle folder selection from tree or breadcrumb"""
        # Update filter panel display
        self.filter_panel.set_selected_folder(folder_path)

        # Update breadcrumb
        breadcrumb = self.service_bridge.get_folder_breadcrumb(folder_id)
        self.center_panel.breadcrumb.set_path(breadcrumb)

        # Trigger cross-indexed search
        self._current_folder_id = folder_id
        self._execute_search()

    def _on_clear_folder(self):
        """Clear folder filter"""
        self._current_folder_id = None
        self.filter_panel.set_selected_folder(None)
        self.center_panel.breadcrumb.clear()
        self.directory_panel.clear_selection()
        self._execute_search()

    def _execute_search(self):
        """Execute cross-indexed search with current parameters"""
        query = self.filter_panel.search_box.text()
        filters = self.filter_panel.get_filters()

        self.service_bridge.search_debounced(
            query=query,
            folder_id=self._current_folder_id,
            filters=filters
        )
```

---

## 6. Keyboard Shortcuts

### 6.1 Updated Shortcuts

| Shortcut | Action | Notes |
|----------|--------|-------|
| `Ctrl+1` | Toggle Directory Browser | Was: Toggle Filter Panel |
| `Ctrl+2` | Toggle Filter Panel | Was: Toggle Spotting Panel |
| `Ctrl+F` | Focus Search Box | New |
| `Escape` | Clear folder selection | When focus in directory tree |
| `Left Arrow` | Collapse folder | When folder selected |
| `Right Arrow` | Expand folder | When folder selected |
| `Enter` | Select folder | When folder highlighted |
| `Backspace` | Go to parent folder | When folder selected |

### 6.2 Implementation

```python
# Add to main_window_soundminer.py

def _setup_shortcuts(self):
    # Toggle panels
    QShortcut(QKeySequence("Ctrl+1"), self, self._toggle_directory_panel)
    QShortcut(QKeySequence("Ctrl+2"), self, self._toggle_filter_panel)

    # Focus search
    QShortcut(QKeySequence("Ctrl+F"), self, self._focus_search)

    # Clear folder (Escape when in tree)
    # Handled by tree widget key event

def _toggle_directory_panel(self):
    """Toggle directory browser visibility"""
    visible = self.directory_panel.isVisible()
    self.directory_panel.setVisible(not visible)

def _toggle_filter_panel(self):
    """Toggle filter panel visibility"""
    visible = self.filter_panel.isVisible()
    self.filter_panel.setVisible(not visible)

def _focus_search(self):
    """Focus the search box"""
    self.filter_panel.search_box.setFocus()
    self.filter_panel.search_box.selectAll()
```

---

## 7. Context Menus

### 7.1 Directory Tree Context Menu

```python
# In DirectoryBrowserPanel

def _setup_context_menu(self):
    self.tree.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
    self.tree.customContextMenuRequested.connect(self._show_context_menu)

def _show_context_menu(self, pos):
    item = self.tree.itemAt(pos)
    if not item:
        return

    menu = RoundMenu(self)

    # Scan folder
    scan_action = Action(FIF.SYNC, "Scan Folder")
    scan_action.triggered.connect(lambda: self._scan_folder(item))
    menu.addAction(scan_action)

    # Reveal in Explorer
    reveal_action = Action(FIF.FOLDER, "Reveal in Explorer")
    reveal_action.triggered.connect(lambda: self._reveal_folder(item))
    menu.addAction(reveal_action)

    menu.addSeparator()

    # Remove from library
    remove_action = Action(FIF.DELETE, "Remove from Library")
    remove_action.triggered.connect(lambda: self._remove_folder(item))
    menu.addAction(remove_action)

    menu.exec(self.tree.mapToGlobal(pos))
```

---

## 8. Performance Optimizations

### 8.1 Lazy Loading Tree

- Only load root folders on startup
- Load children when folder expanded
- Use placeholder "Loading..." item
- Cache loaded folders in memory

### 8.2 Debounced Updates

- Filter changes debounced by 300ms
- Folder counts updated asynchronously
- Results table uses virtual model

### 8.3 Memory Management

- Clear children of collapsed folders after 5 minutes
- Limit maximum tree depth display (show "..." for very deep)
- Virtual scrolling for large result sets

---

## 9. Visual Design Specifications

### 9.1 Colors (Dark Theme)

| Element | Color | Hex |
|---------|-------|-----|
| Panel background | Dark gray | #1E1E1E |
| Card background | Slightly lighter | #2D2D2D |
| Folder icon | Yellow | #FFD93D |
| Selected folder | Accent blue | #0078D4 |
| File count text | Gray | #808080 |
| Breadcrumb separator | Gray | #606060 |

### 9.2 Spacing

| Element | Value |
|---------|-------|
| Panel margins | 16px |
| Component spacing | 12px |
| Tree indentation | 20px |
| Button padding | 8px 16px |

---

## 10. Implementation Checklist

- [ ] Create `DirectoryBrowserPanel` component
- [ ] Create `BreadcrumbWidget` component
- [ ] Modify `FilterPanel` (relocate to right)
- [ ] Update `CenterResultsPanel` (add breadcrumb, folder column)
- [ ] Update `AudioLibMainWindow` (new layout)
- [ ] Add keyboard shortcuts
- [ ] Add context menus
- [ ] Connect to ServiceBridge
- [ ] Test lazy loading performance
- [ ] Test with 1M+ records
