for T in N46_00_E010 N46_00_E011 N46_00_E012 N47_00_E010 N47_00_E011 N47_00_E012 N48_00_E010 N48_00_E011 N48_00_E012; do                                                   
    NAME="Copernicus_DSM_COG_10_${T}_00_DEM"                                                                                                                     
    mkdir "tiles/$NAME"                                                                                                                                          
    curl -L "https://copernicus-dem-30m.s3.amazonaws.com/${NAME}/${NAME}.tif" -o "tiles/${NAME}/${NAME}.tif"                                                     
  done
