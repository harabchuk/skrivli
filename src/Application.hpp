#pragma once

#include "MainWindow.hpp"

class Application {
public:
    int Run(HINSTANCE instance, int showCommand);

private:
    MainWindow m_mainWindow;
};
