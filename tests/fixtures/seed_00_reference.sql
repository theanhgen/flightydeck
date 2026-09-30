-- Synthetic reference data. Public airport/airline facts, invented ids.
INSERT INTO Airport (id,name,iata,icao,city,country,countryCode,timeZoneIdentifier,latitude,longitude,relevance,created,lastUpdated) VALUES
 ('a0000000-0000-4000-8000-000000000001','Václav Havel Prague','PRG','LKPR','Prague','Czech Republic','CZ','Europe/Prague',50.1008,14.26,100,0,0),
 ('a0000000-0000-4000-8000-000000000002','Hamad International','DOH','OTHH','Doha','Qatar','QA','Asia/Qatar',25.2731,51.6081,100,0,0),
 ('a0000000-0000-4000-8000-000000000003','Nội Bài International','HAN','VVNB','Hà Nội','Viet Nam','VN','Asia/Ho_Chi_Minh',21.2212,105.8072,100,0,0),
 ('a0000000-0000-4000-8000-000000000004','Tân Sơn Nhất International','SGN','VVTS','Hồ Chí Minh City','Viet Nam','VN','Asia/Ho_Chi_Minh',10.8188,106.6519,100,0,0),
 ('a0000000-0000-4000-8000-000000000005','Côn Đảo Airport','VCS','VVCS','Côn Đảo','Viet Nam','VN','Asia/Ho_Chi_Minh',8.7319,106.6325,10,0,0),
 ('a0000000-0000-4000-8000-000000000006','Zürich Airport','ZRH','LSZH','Zürich','Switzerland','CH','Europe/Zurich',47.4647,8.5492,100,0,0);

INSERT INTO Airline (id,name,iata,icao,alliance,relevance,created,lastUpdated) VALUES
 ('b0000000-0000-4000-8000-000000000001','Qatar Airways','QR','QTR','Oneworld',100,0,0),
 ('b0000000-0000-4000-8000-000000000002','Qatar Executive','QR','QQE',NULL,50,0,0),
 ('b0000000-0000-4000-8000-000000000003','Vietnam Airlines','VN','HVN','SkyTeam',100,0,0),
 ('b0000000-0000-4000-8000-000000000004','Swiss','LX','SWR','Star Alliance',100,0,0);

INSERT INTO AircraftType (id,name,iata,icao,manufacturer,relevance,lastUpdated,created) VALUES
 ('c0000000-0000-4000-8000-000000000001','Airbus A350-900','359','A359','Airbus',100,0,0),
 ('c0000000-0000-4000-8000-000000000002','Airbus A321','321','A321','Airbus',100,0,0);
