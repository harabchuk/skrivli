#pragma once

#include "views/MainWindow.hpp"

#include <windows.h>

#include <wil/resource.h>

class Application {
  public:
    int Run(HINSTANCE instance, int showCommand);

  private:
    bool ClaimSingleInstance();
    void ActivateExistingInstance() const;
    void ReleaseSingleInstance();
    static bool HasTrayArgument();

    MainWindow m_mainWindow;
    wil::unique_handle m_instanceMutex;
};
