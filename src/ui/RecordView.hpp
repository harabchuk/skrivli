#pragma once

#include "pch.hpp"
#include "Ids.hpp"

class RecordView
    : public CDialogImpl<RecordView>
    , public WTL::CDialogResize<RecordView> {
public:
    enum { IDD = IDD_RECORD_PAGE };

    BEGIN_MSG_MAP_EX(RecordView)
        MSG_WM_INITDIALOG(OnInitDialog)
        CHAIN_MSG_MAP(WTL::CDialogResize<RecordView>)
    END_MSG_MAP()

    BEGIN_DLGRESIZE_MAP(RecordView)
        DLGRESIZE_CONTROL(IDC_RECORD_HINT, DLSZ_SIZE_X | DLSZ_SIZE_Y)
    END_DLGRESIZE_MAP()

private:
    BOOL OnInitDialog(CWindow /*focus*/, LPARAM /*initParam*/) {
        DlgResize_Init(false, false);
        return TRUE;
    }
};
