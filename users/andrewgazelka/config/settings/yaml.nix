# YAML application configuration. Nix is the canonical source.
{
  k9s-config = {
    k9s = {
      ui = {
        skin = "intellij";
        noIcons = false;
        headless = true;
        enableMouse = false;
      };
    };
  };
  k9s-skins-main = {
    k9s = {
      body = {
        fgColor = "#4a5568";
        bgColor = "#ffffff";
        logoColor = "#cbd5e0";
      };
      help = {
        fgColor = "#4a5568";
        bgColor = "#ffffff";
        keyColor = "#718096";
        numKeyColor = "#718096";
        sectionColor = "#4a5568";
      };
      frame = {
        border = {
          fgColor = "#e2e8f0";
          focusColor = "#cbd5e0";
        };
        menu = {
          fgColor = "#4a5568";
          keyColor = "#718096";
          numKeyColor = "#718096";
        };
        crumbs = {
          fgColor = "#4a5568";
          bgColor = "#ffffff";
          activeColor = "#718096";
        };
        status = {
          newColor = "#68d391";
          modifyColor = "#f6ad55";
          addColor = "#90cdf4";
          pendingColor = "#b794f4";
          errorColor = "#fc8181";
          highlightColor = "#90cdf4";
          killColor = "#cbd5e0";
          completedColor = "#81e6d9";
        };
        title = {
          fgColor = "#4a5568";
          bgColor = "#ffffff";
          highlightColor = "#718096";
          counterColor = "#a0aec0";
          filterColor = "#a0aec0";
        };
      };
      views = {
        charts = {
          bgColor = "#ffffff";
          defaultDialColors = [
            "#cbd5e0"
            "#e2e8f0"
          ];
          defaultChartColors = [
            "#cbd5e0"
            "#e2e8f0"
          ];
        };
        table = {
          fgColor = "#4a5568";
          bgColor = "#ffffff";
          cursorFgColor = "#2d3748";
          cursorBgColor = "#f7fafc";
          markColor = "#e2e8f0";
          header = {
            fgColor = "#4a5568";
            bgColor = "#fafafa";
            sorterColor = "#718096";
          };
        };
        xray = {
          fgColor = "#4a5568";
          bgColor = "#ffffff";
          cursorColor = "#e2e8f0";
          cursorTextColor = "#2d3748";
          graphicColor = "#e2e8f0";
        };
        yaml = {
          keyColor = "#718096";
          colonColor = "#a0aec0";
          valueColor = "#4a5568";
        };
        logs = {
          fgColor = "#4a5568";
          bgColor = "#ffffff";
          indicator = {
            fgColor = "#4a5568";
            bgColor = "#f7fafc";
            toggleOnColor = "#68d391";
            toggleOffColor = "#fc8181";
          };
        };
      };
      info = {
        fgColor = "#4a5568";
        sectionColor = "#718096";
      };
      dialog = {
        fgColor = "#4a5568";
        bgColor = "#ffffff";
        buttonFgColor = "#4a5568";
        buttonBgColor = "#f7fafc";
        buttonFocusFgColor = "#2d3748";
        buttonFocusBgColor = "#edf2f7";
        labelFgColor = "#718096";
        fieldFgColor = "#4a5568";
      };
      status = {
        running = {
          fgColor = "#68d391";
        };
        pending = {
          fgColor = "#f6ad55";
        };
        succeeded = {
          fgColor = "#81e6d9";
        };
        failed = {
          fgColor = "#fc8181";
        };
        unknown = {
          fgColor = "#b794f4";
        };
        containerCreating = {
          fgColor = "#90cdf4";
        };
        containerTerminated = {
          fgColor = "#a0aec0";
        };
        containerReady = {
          fgColor = "#68d391";
        };
        containerNotReady = {
          fgColor = "#fc8181";
        };
        loadBalancer = {
          fgColor = "#90cdf4";
        };
        nodePort = {
          fgColor = "#81e6d9";
        };
        clusterIP = {
          fgColor = "#b794f4";
        };
        ready = {
          fgColor = "#68d391";
        };
        notReady = {
          fgColor = "#fc8181";
        };
        available = {
          fgColor = "#68d391";
        };
        unavailable = {
          fgColor = "#fc8181";
        };
      };
    };
  };
  process-compose-process-compose = {
    version = "0.5";
    log_level = "info";
    log_length = 1000;
    environment = [
      "LOG_LEVEL=info"
    ];
    processes = null;
  };
  process-compose-theme = {
    style = {
      body = {
        fgColor = "black";
        bgColor = "white";
        secondaryTextColor = "#0066CC";
        tertiaryTextColor = "#008800";
        borderColor = "#666666";
      };
      stat_table = {
        keyFgColor = "#0066CC";
        valueFgColor = "black";
        logoColor = "#0066CC";
      };
      proc_table = {
        fgColor = "black";
        bgColor = "white";
        headerFgColor = "#0066CC";
        headerBgColor = "#E8E8E8";
      };
      dialog = {
        fgColor = "black";
        bgColor = "white";
        buttonBgColor = "#0066CC";
        buttonFgColor = "white";
        labelFgColor = "#666666";
        fieldBgColor = "#F5F5F5";
      };
      help = {
        fgColor = "black";
        bgColor = "white";
        keyFgColor = "#0066CC";
        categoryFgColor = "#666666";
      };
    };
  };
}
