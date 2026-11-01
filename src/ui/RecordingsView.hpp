#pragma once

#include "pch.hpp"
#include "Ids.hpp"

class RecordingsView
    : public CDialogImpl<RecordingsView>
    , public WTL::CDialogResize<RecordingsView> {
public:
    enum { IDD = IDD_RECORDINGS_PAGE };

    BEGIN_MSG_MAP_EX(RecordingsView)
        MSG_WM_INITDIALOG(OnInitDialog)
        CHAIN_MSG_MAP(WTL::CDialogResize<RecordingsView>)
    END_MSG_MAP()

    BEGIN_DLGRESIZE_MAP(RecordingsView)
        DLGRESIZE_CONTROL(IDC_RECORDINGS_HINT, DLSZ_SIZE_X | DLSZ_SIZE_Y)
    END_DLGRESIZE_MAP()

private:
    BOOL OnInitDialog(CWindow /*focus*/, LPARAM /*initParam*/) {
        DlgResize_Init(false, false);
        return TRUE;
    }
};
