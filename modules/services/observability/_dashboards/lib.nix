{pkgs}: let
  inherit (pkgs) lib;
  gridColumns = 24;
  clickhouseDatasource = {
    type = "grafana-clickhouse-datasource";
    uid = "ix-clickhouse";
  };
  paletteClassic.color.mode = "palette-classic";
  legendRight.legend = {
    displayMode = "table";
    placement = "right";
  };
  isNumber = value: builtins.isInt value || builtins.isFloat value;
  assertPositiveInt = name: value:
    assert lib.assertMsg (
      builtins.isInt value && value > 0
    ) "Grafana dashboard field `${name}` must be a positive integer."; value;
  assertNumberOrNull = name: value:
    assert lib.assertMsg (
      value == null || isNumber value
    ) "Grafana dashboard field `${name}` must be a number or null."; value;
  layoutItemPanel = item: item.panel or item;
  layoutItemSpan = item:
    if item ? span
    then assertPositiveInt "layout row panel span" item.span
    else 1;
  layoutRow = state: row: let
    height = assertPositiveInt "layout row height" row.height;
    totalSpan = builtins.foldl' (total: item: total + layoutItemSpan item) 0 row.panels;
    unitWidth = builtins.div gridColumns totalSpan;
    rowState = assert lib.assertMsg (totalSpan > 0) "Grafana dashboard layout rows must contain panels.";
    assert lib.assertMsg (unitWidth * totalSpan == gridColumns)
    "Grafana dashboard layout row span total ${toString totalSpan} must divide ${toString gridColumns} grid columns.";
      builtins.foldl'
      (
        rowAccumulator: item: let
          panel = layoutItemPanel item;
          span = layoutItemSpan item;
          gridPos = {
            h = height;
            w = span * unitWidth;
            x = rowAccumulator.spanOffset * unitWidth;
            inherit (state) y;
          };
        in {
          spanOffset = rowAccumulator.spanOffset + span;
          panels = rowAccumulator.panels ++ [(panel // {inherit gridPos;})];
        }
      )
      {
        spanOffset = 0;
        panels = [];
      }
      row.panels;
  in {
    y = state.y + height;
    panels = state.panels ++ rowState.panels;
  };
  sqlTarget = {
    refId ? "A",
    queryType ? "sql",
    rawSql,
    format ? null,
  }:
    {
      inherit queryType refId rawSql;
    }
    // lib.optionalAttrs (format != null) {inherit format;};
  thresholds = steps: {
    mode = "absolute";
    inherit steps;
  };
  thresholdStep = {
    color,
    value ? null,
  }: {
    inherit color;
    value = assertNumberOrNull "thresholdStep.value" value;
  };
  numericBounds = {
    min ? null,
    max ? null,
  }:
    lib.optionalAttrs (min != null) {min = assertNumberOrNull "min" min;}
    // lib.optionalAttrs (max != null) {max = assertNumberOrNull "max" max;};
  basePanel = {
    id,
    title,
    type,
    targets,
    datasource ? clickhouseDatasource,
    fieldConfig ? null,
    gridPos ? null,
    options ? null,
  }:
    {
      inherit
        datasource
        id
        targets
        title
        type
        ;
    }
    // lib.optionalAttrs (fieldConfig != null) {inherit fieldConfig;}
    // lib.optionalAttrs (gridPos != null) {inherit gridPos;}
    // lib.optionalAttrs (options != null) {inherit options;};
  clickhouseStatPanel = {
    id,
    title,
    rawSql,
    colorMode ? null,
    format ? null,
    gridPos ? null,
    options ? null,
    thresholds ? null,
    unit ? "short",
  }:
    basePanel {
      inherit
        id
        gridPos
        options
        title
        ;
      type = "stat";
      targets = [
        (sqlTarget {
          inherit format rawSql;
        })
      ];
      fieldConfig.defaults =
        {
          inherit unit;
        }
        // lib.optionalAttrs (colorMode != null) {color.mode = colorMode;}
        // lib.optionalAttrs (thresholds != null) {
          thresholds = {
            mode = "absolute";
            steps = thresholds;
          };
        };
    };
  clickhouseTimeseriesPanel = {
    id,
    title,
    rawSql,
    custom ? null,
    format ? null,
    gridPos ? null,
    options ? {},
    unit ? "short",
  }:
    basePanel {
      inherit id gridPos title;
      type = "timeseries";
      targets = [
        (sqlTarget {
          inherit format rawSql;
        })
      ];
      fieldConfig.defaults =
        {
          inherit unit;
        }
        // paletteClassic
        // lib.optionalAttrs (custom != null) {inherit custom;};
      options = legendRight // options;
    };
  clickhouseTablePanel = {
    id,
    title,
    rawSql,
    format ? null,
    gridPos ? null,
    showHeader ? null,
  }:
    basePanel {
      inherit id gridPos title;
      type = "table";
      targets = [
        (sqlTarget {
          inherit format rawSql;
        })
      ];
      options =
        if showHeader == null
        then null
        else {inherit showHeader;};
    };
  clickhouseLogsPanel = {
    id,
    title,
    rawSql,
    gridPos ? null,
    options ? {},
  }:
    basePanel {
      inherit
        id
        gridPos
        options
        title
        ;
      type = "logs";
      targets = [
        (sqlTarget {
          queryType = "logs";
          inherit rawSql;
        })
      ];
      fieldConfig = {
        defaults = {};
        overrides = [];
      };
    };
in {
  json = pkgs.formats.json {};

  inherit
    basePanel
    clickhouseDatasource
    clickhouseLogsPanel
    clickhouseStatPanel
    clickhouseTablePanel
    clickhouseTimeseriesPanel
    legendRight
    numericBounds
    paletteClassic
    sqlTarget
    thresholdStep
    thresholds
    ;

  span = units: panel: {
    span = assertPositiveInt "layout panel span" units;
    inherit panel;
  };

  layoutRows = rows: let
    layout =
      builtins.foldl' layoutRow {
        y = 0;
        panels = [];
      }
      rows;
  in
    layout.panels;
}
