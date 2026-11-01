#pragma once

#include "ui/MainWindow.hpp"

#include <windows.h>

class Application {
public:
    int Run(HINSTANCE instance, int showCommand);

private:
    bool ClaimSingleInstance();
    void ActivateExistingInstance() const;
    void ReleaseSingleInstance();

    MainWindow m_mainWindow;
    HANDLE m_instanceMutex = nullptr;
};
