#include "ui/DeviceCombo.hpp"

void DeviceCombo::Fill(const std::vector<Item>& items) {
    m_combo.ResetContent();
    m_ids.clear();
    m_ids.reserve(items.size());
    for (const auto& item : items) {
        Add(item.label, item.id);
    }
}

int DeviceCombo::Add(const std::wstring& label, const std::wstring& id) {
    const int index = m_combo.AddString(label.c_str());
    if (index >= 0) {
        m_ids.push_back(id);
    }
    return index;
}

bool DeviceCombo::Select(const std::wstring& id) {
    int fallback = 0;
    bool haveFallback = false;
    for (size_t i = 0; i < m_ids.size(); ++i) {
        if (m_ids[i] == id) {
            m_combo.SetCurSel(static_cast<int>(i));
            return true;
        }
        if (!haveFallback && m_ids[i].empty()) {
            fallback = static_cast<int>(i);
            haveFallback = true;
        }
    }
    m_combo.SetCurSel(fallback);
    return false;
}

auto DeviceCombo::SelectedId() const -> std::wstring {
    const int sel = m_combo.GetCurSel();
    if (sel < 0 || static_cast<size_t>(sel) >= m_ids.size()) {
        return {};
    }
    return m_ids[static_cast<size_t>(sel)];
}
