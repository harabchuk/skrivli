#pragma once

#include "pch.hpp"

#include <string>
#include <vector>

// Drop-down list whose rows carry a string id kept beside the control instead of in item data.
class DeviceCombo {
public:
    struct Item {
        std::wstring label;
        std::wstring id;
    };

    void Attach(HWND combo) { m_combo.Attach(combo); }
    bool IsWindow() const { return m_combo.IsWindow(); }
    WTL::CComboBox& Combo() { return m_combo; }
    const WTL::CComboBox& Combo() const { return m_combo; }

    void Fill(const std::vector<Item>& items);
    int Add(const std::wstring& label, const std::wstring& id);
    // Returns false when no row has this id; the selection then falls back to the first row
    // with an empty id, or row 0 when there is none.
    bool Select(const std::wstring& id);
    std::wstring SelectedId() const;

private:
    WTL::CComboBox m_combo;
    std::vector<std::wstring> m_ids;
};
