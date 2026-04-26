for LAT in 45 46 47 48 49; do
  for LON in 9 10 11 12 13; do
    NAME=$(printf "Copernicus_DSM_COG_10_N%02d_00_E%03d_00_DEM" $LAT $LON)
    DEST="tiles/${NAME}/${NAME}.tif"
    if [ -f "$DEST" ]; then
      echo "already have $NAME, skipping"
    else
      mkdir -p "tiles/$NAME"
      echo "downloading $NAME ..."
      curl -L "https://copernicus-dem-30m.s3.amazonaws.com/${NAME}/${NAME}.tif" -o "$DEST"
    fi
  done
done
